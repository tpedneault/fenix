//! A code extension's page: laid out by the extension (`page-render`), as
//! lines of coloured runs, and drawn here as any page is. The last page it
//! gave stays on screen while it works on the next, so a slow extension
//! never freezes Fenix. Keys go to it (`page-key`), except `q` and `Esc`,
//! which close the page as on any other.

use crate::page::{fit, frame, Grid, Key, Page, Role};
use fenix_wasm::host as wit;

pub struct CodePage {
    pub ext: String,
    /// Its id, as the extension knows it.
    pub page: u64,
    pub kind: String,
    pub title: String,
    /// What it last laid out, and for how big a pane: width, height.
    pub shown: Option<wit::Page>,
    pub width: Option<(u32, u32)>,
    /// The size asked for and not yet given.
    pub asked: Option<(u32, u32)>,
    /// Why its extension stopped, when it has.
    pub stopped: Option<String>,
    /// What it last said it needs to open again (`page-state`), kept for
    /// the session.
    pub state: Option<String>,
    /// Opened beside the pane you were in (`page-open-beside`): a file it
    /// opens goes there, not in its own pane.
    pub beside: bool,
    /// A surface's pixels, when it's one (`surface-open`) rather than a
    /// page it lays out.
    pub surface: Option<CodeSurface>,
    /// The file it shows, when it opened (or came forward) as its
    /// extension was told it opens one (`opened`): `SPC f o` goes to its
    /// text.
    pub file: Option<std::path::PathBuf>,
}

/// A code extension's surface, as Fenix draws it: its pixels shared with
/// its worker, their texture, the pointer over it.
pub struct CodeSurface {
    pub shared: wit::SharedSurface,
    pub options: wit::SurfaceOptions,
    pub texture: Option<crate::surface::Surface>,
    /// The pointer's picture, and its texture once made.
    pub cursor: Option<(wit::CursorImage, Option<crate::surface::Surface>)>,
    /// Where the pointer is over it, in its pixels, and the buttons held.
    pub pointer: Option<(i32, i32)>,
    pub buttons: u8,
    /// The pane size it was last told (`surface-resized`), and the one
    /// it's been drawn at since when.
    pub told: Option<(u32, u32, f32)>,
    pub seen: Option<((u32, u32, f32), std::time::Instant)>,
}

impl CodeSurface {
    pub fn new(shared: wit::SharedSurface, options: wit::SurfaceOptions) -> Self {
        CodeSurface { shared, options, texture: None, cursor: None, pointer: None, buttons: 0, told: None, seen: None }
    }

    /// Where its pixels go in a pane at `rect` (x, y, width, height), and
    /// the part of them shown there (`(u0, v0, u1, v1)`).
    pub fn placed(&self, rect: (f32, f32, f32, f32)) -> Option<((f32, f32, f32, f32), (f32, f32, f32, f32))> {
        placed(self.options.scale, rect, self.shared.lock().size())
    }

    /// Pane pixel `pos` (from the window's top left) as one of its own,
    /// for a pane at `rect`; none outside what's drawn of it, unless
    /// `clamp`ed to its edge (a drag that went past it).
    pub fn to_pixels(&self, rect: (f32, f32, f32, f32), pos: (f32, f32), clamp: bool) -> Option<(i32, i32)> {
        let size = self.shared.lock().size();
        let ((x, y, w, h), (u0, v0, u1, v1)) = placed(self.options.scale, rect, size)?;
        let (fx, fy) = ((pos.0 - x) / w, (pos.1 - y) / h);
        if !clamp && !((0.0..1.0).contains(&fx) && (0.0..1.0).contains(&fy)) {
            return None;
        }
        let (fx, fy) = (fx.clamp(0.0, 1.0), fy.clamp(0.0, 1.0));
        let px = ((u0 + fx * (u1 - u0)) * size.0 as f32).floor().min(size.0.saturating_sub(1) as f32);
        let py = ((v0 + fy * (v1 - v0)) * size.1 as f32).floor().min(size.1.saturating_sub(1) as f32);
        Some((px as i32, py as i32))
    }
}

/// A surface of `size` pixels in a pane at `rect`, as `scale` fits it:
/// where it's drawn, and which part of it.
pub fn placed(scale: wit::SurfaceScale, (x, y, w, h): (f32, f32, f32, f32), size: (u32, u32)) -> Option<((f32, f32, f32, f32), (f32, f32, f32, f32))> {
    let (sw, sh) = (size.0 as f32, size.1 as f32);
    if sw < 1.0 || sh < 1.0 || w < 1.0 || h < 1.0 {
        return None;
    }
    Some(match scale {
        wit::SurfaceScale::Stretch => ((x, y, w, h), (0.0, 0.0, 1.0, 1.0)),
        wit::SurfaceScale::Fit => {
            let s = (w / sw).min(h / sh);
            ((x, y, (sw * s).round(), (sh * s).round()), (0.0, 0.0, 1.0, 1.0))
        }
        wit::SurfaceScale::None => {
            let (dw, dh) = (sw.min(w), sh.min(h));
            ((x, y, dw, dh), (0.0, 0.0, dw / sw, dh / sh))
        }
    })
}

/// A key as a surface hears it; none for one it has no name for.
pub fn surface_key(keypress: fenix_keymap::KeyPress, down: bool) -> Option<wit::KeyEvent> {
    use fenix_keymap::{KeyCode, NamedKey as N};
    let key = match keypress.code {
        KeyCode::Char(c) => wit::SurfaceKey::Char(c),
        KeyCode::Named(n) => wit::SurfaceKey::Named(match n {
            N::Escape => wit::NamedKey::Escape,
            N::Enter => wit::NamedKey::Enter,
            N::Tab => wit::NamedKey::Tab,
            N::Backspace => wit::NamedKey::Backspace,
            N::Delete => wit::NamedKey::Delete,
            N::Left => wit::NamedKey::Left,
            N::Right => wit::NamedKey::Right,
            N::Up => wit::NamedKey::Up,
            N::Down => wit::NamedKey::Down,
            N::Home => wit::NamedKey::Home,
            N::End => wit::NamedKey::End,
            N::PageUp => wit::NamedKey::PageUp,
            N::PageDown => wit::NamedKey::PageDown,
        }),
    };
    Some(wit::KeyEvent { key, down, mods: modifiers(keypress.mods) })
}

/// A key for a page that takes more keys (`page.more-keys`), when
/// it's one `page-key` doesn't take and Fenix keeps for itself nowhere:
/// PageUp and PageDown, Shift with a key that has a name, Ctrl, Alt or
/// the Windows key with others -- but not Fenix's own chords that work
/// from anywhere (Ctrl+Q, the font's size, Ctrl+O and Ctrl+I, the tab
/// keys, Ctrl+\\), nor those `page-key` has while a field is typed in.
pub fn key_press(keypress: fenix_keymap::KeyPress, shift: bool, typing: bool) -> Option<wit::KeyPress> {
    use fenix_keymap::{KeyCode, NamedKey as N};
    let m = keypress.mods;
    let take = match keypress.code {
        KeyCode::Named(N::PageUp | N::PageDown) if m.ctrl => false,
        KeyCode::Named(N::Tab) if m.ctrl => false,
        KeyCode::Char(c) if m.ctrl && !m.alt && matches!(c.to_ascii_lowercase(), 'q' | '=' | '+' | '-' | '0' | 'i' | '\\') => false,
        KeyCode::Char(c) if m.ctrl && !m.alt && c.eq_ignore_ascii_case(&'o') && !typing => false,
        // `page-key`'s, as it has them.
        KeyCode::Char('c') if m.ctrl && !m.alt && !shift => false,
        KeyCode::Char('o' | 'w') if m.ctrl && !m.alt && !shift && typing => false,
        KeyCode::Named(N::Left | N::Right | N::Backspace) if m.ctrl && !m.alt && !shift && typing => false,
        // Shift+Tab is `back-tab`.
        KeyCode::Named(N::Tab) => false,
        KeyCode::Named(N::PageUp | N::PageDown) => true,
        KeyCode::Named(N::Home | N::End | N::Delete) if !typing => true,
        KeyCode::Named(_) if shift || m.alt || m.super_ || m.ctrl => true,
        KeyCode::Char(_) => m.ctrl || m.alt || m.super_,
        _ => false,
    };
    if !take {
        return None;
    }
    let key = surface_key(keypress, true)?.key;
    Some(wit::KeyPress { key, mods: wit::KeyMods { shift, ctrl: m.ctrl, alt: m.alt, meta: m.super_ } })
}

pub fn modifiers(mods: fenix_keymap::Mods) -> wit::Modifiers {
    wit::Modifiers { ctrl: mods.ctrl, alt: mods.alt, meta: mods.super_ }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Close,
    Key(wit::Key),
}

impl CodePage {
    pub fn new(ext: &str, page: u64, kind: &str, title: &str) -> Self {
        CodePage { ext: ext.into(), page, kind: kind.into(), title: title.into(), shown: None, width: None, asked: None, stopped: None, state: None, surface: None, beside: false, file: None }
    }

    /// What it's called: its title, else its kind.
    pub fn name(&self) -> &str {
        if self.title.trim().is_empty() { &self.kind } else { &self.title }
    }

    /// The boxes its canvas outlines over its pictures, by picture.
    pub fn image_marks(&self) -> Vec<(usize, (f32, f32, f32, f32), bool)> {
        let Some(canvas) = self.shown.as_ref().and_then(|p| p.canvas.as_ref()) else { return Vec::new() };
        canvas.pictures.iter().enumerate().flat_map(|(i, p)| p.marks.iter().map(move |m| (i, (m.x, m.y, m.width, m.height), m.selected))).collect()
    }

    /// Whether a field on it is being typed in: then every key is its.
    pub fn typing(&self) -> bool {
        self.shown.as_ref().is_some_and(|p| p.typing)
    }

    /// Whether it takes the keys `page-key` doesn't (`page-key-press`).
    pub fn more_keys(&self) -> bool {
        self.stopped.is_none() && self.shown.as_ref().is_some_and(|p| p.more_keys)
    }

    /// Whether clicks and the wheel are its (`page-mouse`).
    pub fn mouse(&self) -> bool {
        self.stopped.is_none() && self.shown.as_ref().is_some_and(|p| p.mouse)
    }

    /// Whether Space is its key, not the leader.
    pub fn claims_space(&self) -> bool {
        self.typing() || self.shown.as_ref().is_some_and(|p| p.space)
    }

    pub fn key(&self, key: Key) -> Action {
        // While its popup is open, or it's typing, `q` and `Esc` are the
        // extension's: they close the popup, not the page.
        let popup = self.shown.as_ref().is_some_and(|p| p.popup.is_some() || p.canvas.as_ref().is_some_and(|c| c.popup.is_some()));
        match key {
            Key::Char('q') | Key::Escape if !popup && !self.typing() => Action::Close,
            Key::Space if self.claims_space() => Action::Key(wit::Key::Char(' ')),
            other => to_wit(other).map_or(Action::None, Action::Key),
        }
    }
}

/// A page key as the extension API has it; Space is the leader.
fn to_wit(key: Key) -> Option<wit::Key> {
    Some(match key {
        Key::Up => wit::Key::Up,
        Key::Down => wit::Key::Down,
        Key::Left => wit::Key::Left,
        Key::Right => wit::Key::Right,
        Key::Enter => wit::Key::Enter,
        Key::Tab => wit::Key::Tab,
        Key::BackTab => wit::Key::BackTab,
        Key::Escape => wit::Key::Escape,
        Key::Backspace => wit::Key::Backspace,
        Key::Delete => wit::Key::Delete,
        Key::Home => wit::Key::Home,
        Key::End => wit::Key::End,
        Key::WordLeft => wit::Key::WordLeft,
        Key::WordRight => wit::Key::WordRight,
        Key::DeleteWordBack => wit::Key::DeleteWordBack,
        Key::Char(c) => wit::Key::Char(c),
        _ => return None,
    })
}

pub(crate) fn role(r: &wit::Role) -> Role {
    match r {
        wit::Role::Title => Role::Title,
        wit::Role::Text => Role::Text,
        wit::Role::Muted => Role::Muted,
        wit::Role::Accent => Role::Accent,
        wit::Role::Good => Role::Good,
        wit::Role::Warn => Role::Warn,
        wit::Role::Bad => Role::Bad,
        wit::Role::Syntax(name) => Role::Syntax(syntax_name(name)),
    }
}

/// A syntax capture's name, kept for as long as Fenix runs: there are
/// only so many.
fn syntax_name(name: &str) -> &'static str {
    static NAMES: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let mut names = NAMES.lock().unwrap();
    if let Some(known) = names.iter().find(|n| **n == name) {
        return known;
    }
    if names.len() >= 256 {
        return "text";
    }
    let kept: &'static str = Box::leak(name.to_string().into_boxed_str());
    names.push(kept);
    kept
}

/// The width an extension lays its page out to, for a pane `cols` wide.
pub fn content_width(cols: usize) -> u32 {
    frame(cols, 120).1 as u32
}

pub(crate) fn shade(s: &wit::Shade) -> crate::page::Shade {
    use crate::page::Shade as S;
    match s {
        wit::Shade::Panel => S::Panel,
        wit::Shade::Focus => S::Focus,
        wit::Shade::Selection => S::Selection,
        wit::Shade::Accent => S::Accent,
        wit::Shade::Good => S::Good,
        wit::Shade::Warn => S::Warn,
        wit::Shade::Bad => S::Bad,
        wit::Shade::Rgb((r, g, b)) => S::Rgb(*r, *g, *b),
    }
}

/// `spans`, with `colors` over them: a span's cells a colour takes are
/// cut out of it (a character is one colour), and the colour's own span
/// added.
fn colored(mut spans: Vec<crate::page::Span>, colors: impl Iterator<Item = (usize, std::ops::Range<usize>, wit::Rgb)>) -> Vec<crate::page::Span> {
    use crate::page::Span;
    for (line, cols, (r, g, b)) in colors {
        if cols.is_empty() {
            continue;
        }
        let mut kept = Vec::with_capacity(spans.len() + 1);
        for s in spans {
            if s.line != line || s.cols.end <= cols.start || s.cols.start >= cols.end {
                kept.push(s);
                continue;
            }
            if s.cols.start < cols.start {
                kept.push(Span { line, cols: s.cols.start..cols.start, role: s.role });
            }
            if s.cols.end > cols.end {
                kept.push(Span { line, cols: cols.end..s.cols.end, role: s.role });
            }
        }
        kept.push(Span { line, cols, role: Role::Rgb(r, g, b) });
        spans = kept;
    }
    spans
}

/// A canvas, as Fenix's own pages are laid out: drawn as it is, with what
/// its page has besides (`page`'s styles, rules, focuses and colours).
fn canvas(ext: &str, c: &wit::Canvas, page: &wit::Page) -> Page {
    use crate::page::{PageImage, Span};
    let cells = |c: &wit::Cells| (c.line as usize, c.start as usize..c.end.max(c.start) as usize);
    let (styles, colors) = (&page.styles, &page.colors);
    Page {
        backgrounds: styles.iter().filter_map(|s| s.background.as_ref().map(|b| { let (line, cols) = cells(&s.cells); (line, cols, shade(b)) })).collect(),
        underlines: styles.iter().filter_map(|s| s.underline.as_ref().map(|r| { let (line, cols) = cells(&s.cells); (line, cols, role(r)) })).collect(),
        vertical_rules: page.vertical_rules.iter().map(|v| (v.column as usize, v.top as usize..v.bottom.max(v.top) as usize)).collect(),
        focuses: page.focuses.iter().map(cells).collect(),
        text: c.text.clone(),
        spans: colored(c.spans.iter().map(|s| { let (line, cols) = cells(&s.cells); Span { line, cols, role: role(&s.role) } }).collect(), colors.iter().map(|c| { let (line, cols) = cells(&c.cells); (line, cols, c.color) })),
        rules: c.rules.iter().map(cells).collect(),
        focus: c.focus.as_ref().map(cells),
        panels: c.panels.iter().map(cells).collect(),
        popup: c.popup.as_ref().map(|p| crate::page::Popup { line: p.line as usize, col: p.column as usize, rows: p.rows.iter().map(|row| row.iter().map(|s| (s.text.clone(), role(&s.role))).collect()).collect() }),
        images: c
            .pictures
            .iter()
            .map(|p| {
                let (line, cols) = cells(&p.cells);
                PageImage { line, col: cols.start, rows: p.rows as usize, cols: cols.len(), key: crate::app::code_image_key(ext, p.image), view: p.view }
            })
            .collect(),
    }
}

pub fn layout(p: &CodePage, cols: usize) -> Page {
    if let (Some(c), None) = (p.shown.as_ref().and_then(|s| s.canvas.as_ref()), &p.stopped) {
        return canvas(&p.ext, c, p.shown.as_ref().expect("a canvas is a shown page's"));
    }
    // A surface's pixels are drawn over its pane, with no text of its own.
    if p.surface.is_some() && p.stopped.is_none() {
        return Page::default();
    }
    let (left, width) = frame(cols, 120);
    let mut g = Grid::new();
    g.put(1, left, &fit(p.name(), width), Role::Title);
    g.rule(2, left..left + width);
    let top = 4;
    match (&p.shown, &p.stopped) {
        (_, Some(why)) => {
            g.put(top, left + 2, &fit(&format!("{why} -- SPC h l has what it said."), width.saturating_sub(4)), Role::Warn);
        }
        (None, None) => {
            g.put(top, left + 2, "Waiting for the extension…", Role::Muted);
        }
        (Some(page), None) => {
            for (i, line) in page.lines.iter().enumerate() {
                let y = top + i;
                if line.rule {
                    g.rule(y, left..left + width);
                    continue;
                }
                if page.focus == Some(i as u32) {
                    g.focus(y, left..left + width);
                }
                let mut x = left;
                for span in &line.spans {
                    let room = (left + width).saturating_sub(x);
                    if room == 0 {
                        break;
                    }
                    x = g.put(y, x, &fit(&span.text, room), role(&span.role));
                }
            }
            if let Some(popup) = &page.popup {
                let rows = popup.rows.iter().map(|row| row.iter().map(|s| (s.text.clone(), role(&s.role))).collect()).collect();
                g.popup = Some(crate::page::Popup { line: top + popup.line as usize, col: left + 2, rows });
            }
        }
    }
    let mut keys: Vec<(&str, &str)> = p.shown.iter().flat_map(|page| page.keys.iter().map(|k| (k.keys.as_str(), k.what.as_str()))).collect();
    keys.push(("q", "close"));
    g.keys(left, width, &keys);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(text: &str, role: wit::Role) -> wit::Span {
        wit::Span { text: text.into(), role }
    }

    /// A surface's pixels in its pane, as its scale has them, and the
    /// pointer back in its pixels.
    #[test]
    fn a_surface_is_placed_and_the_pointer_found_on_it() {
        use wit::SurfaceScale as S;
        let pane = (10.0, 20.0, 400.0, 400.0);
        assert_eq!(placed(S::Stretch, pane, (200, 100)), Some((pane, (0.0, 0.0, 1.0, 1.0))));
        assert_eq!(placed(S::Fit, pane, (200, 100)), Some(((10.0, 20.0, 400.0, 200.0), (0.0, 0.0, 1.0, 1.0))), "its shape kept");
        assert_eq!(placed(S::None, pane, (800, 100)), Some(((10.0, 20.0, 400.0, 100.0), (0.0, 0.0, 0.5, 1.0))), "cut, a pixel a pixel");
        assert_eq!(placed(S::Fit, pane, (0, 0)), None, "nothing to draw yet");
        let shared = wit::SharedSurface::default();
        let surface = CodeSurface::new(shared.clone(), wit::SurfaceOptions { capture: false, scale: S::Fit, beside: None });
        assert_eq!(surface.to_pixels(pane, (100.0, 100.0), false), None, "no size yet");
        // Sized by its worker: 200 x 100, presented; fitted, a pane pixel
        // is half a pixel of it.
        {
            let mut fb = shared.lock();
            fb.resize(200, 100).unwrap();
            fb.present();
        }
        assert_eq!(surface.to_pixels(pane, (10.0 + 100.0, 20.0 + 50.0), false), Some((50, 25)));
        assert_eq!(surface.to_pixels(pane, (10.0 + 100.0, 20.0 + 300.0), false), None, "below what's drawn");
        assert_eq!(surface.to_pixels(pane, (10.0 + 100.0, 20.0 + 300.0), true), Some((50, 99)), "a drag past it, at its edge");
        let key = surface_key(fenix_keymap::KeyPress::char('a').with_ctrl(), false).unwrap();
        assert_eq!((key.key, key.down, key.mods.ctrl), (wit::SurfaceKey::Char('a'), false, true));
        assert_eq!(surface_key(fenix_keymap::KeyPress::named(fenix_keymap::NamedKey::PageDown), true).map(|k| k.key), Some(wit::SurfaceKey::Named(wit::NamedKey::PageDown)));
    }

    /// More keys reach a page that takes them: PageDown, Shift with a
    /// movement, Ctrl+Z and Ctrl+Shift+Z, Ctrl+A; not what `page-key` has
    /// already, nor Fenix's own chords.
    #[test]
    fn a_page_that_takes_more_keys_gets_them_but_not_fenixs() {
        use fenix_keymap::{KeyPress as K, NamedKey as N};
        let press = |k: K, shift: bool| key_press(k, shift, false).map(|p| (p.key, p.mods.shift, p.mods.ctrl));
        assert_eq!(press(K::named(N::PageDown), false), Some((wit::SurfaceKey::Named(wit::NamedKey::PageDown), false, false)));
        assert_eq!(press(K::named(N::Left), true), Some((wit::SurfaceKey::Named(wit::NamedKey::Left), true, false)));
        assert_eq!(press(K::char('z').with_ctrl(), false), Some((wit::SurfaceKey::Char('z'), false, true)));
        assert_eq!(press(K::char('z').with_ctrl(), true), Some((wit::SurfaceKey::Char('z'), true, true)));
        assert_eq!(press(K::char('a').with_ctrl(), false), Some((wit::SurfaceKey::Char('a'), false, true)));
        assert_eq!(press(K::named(N::Left), false), None, "page-key's");
        assert_eq!(press(K::char('a'), false), None, "page-key's");
        assert_eq!(press(K::char(' '), false), None, "the leader");
        assert_eq!(press(K::char('c').with_ctrl(), false), None, "page-key's Ctrl+C");
        assert_eq!(press(K::named(N::Tab), true), None, "back-tab");
        for fenixs in [K::char('q').with_ctrl(), K::char('=').with_ctrl(), K::char('o').with_ctrl(), K::named(N::PageDown).with_ctrl()] {
            assert_eq!(press(fenixs, false), None, "{fenixs:?}");
        }
        assert_eq!(key_press(K::named(N::Left).with_ctrl(), false, true), None, "a word left, while typing");
        assert!(key_press(K::named(N::Left).with_ctrl(), true, true).is_some(), "selecting a word left");
    }

    #[test]
    fn its_lines_are_drawn_with_their_colours_and_focus() {
        let mut p = CodePage::new("test.hello", 1, "counter", "Hello");
        assert!(layout(&p, 100).all_text().contains("Waiting for the extension"));
        p.shown = Some(wit::Page {
            lines: vec![
                wit::Line { spans: vec![span("Containers", wit::Role::Title), span("  3 running", wit::Role::Muted)], rule: false },
                wit::Line { spans: Vec::new(), rule: true },
                wit::Line { spans: vec![span("web", wit::Role::Good)], rule: false },
            ],
            focus: Some(2),
            keys: vec![wit::KeyHint { keys: "s".into(), what: "stop".into() }],
            popup: Some(wit::Popup { line: 2, rows: vec![vec![span("s", wit::Role::Accent), span("  Stop", wit::Role::Text)]] }),
            canvas: None,
            typing: false,
            space: false,
            styles: Vec::new(),
            vertical_rules: Vec::new(),
            focuses: Vec::new(),
            more_keys: false,
            mouse: false,
            colors: Vec::new(),
        });
        let page = layout(&p, 100);
        let text = page.all_text();
        assert!(text.contains("Containers  3 running") && text.contains("web") && text.contains("stop") && text.contains("close"), "{text}");
        assert_eq!(page.focus.as_ref().map(|(line, _)| *line), Some(6));
        let popup = page.popup.as_ref().expect("its menu");
        assert_eq!((popup.line, popup.text().as_str()), (6, "s  Stop"), "beside the line it's about");
        assert_eq!(p.key(Key::Char('s')), Action::Key(wit::Key::Char('s')));
        assert_eq!(p.key(Key::Char('q')), Action::Key(wit::Key::Char('q')), "its popup is open");
        assert_eq!(CodePage::new("x.y", 2, "containers", " ").name(), "containers", "untitled: its kind");
        assert_eq!(p.key(Key::Space), Action::None, "Space is the leader");
        assert_eq!(p.key(Key::Escape), Action::Key(wit::Key::Escape), "the popup's to close, not the page");
        p.shown.as_mut().unwrap().popup = None;
        assert_eq!(p.key(Key::Escape), Action::Close);
        p.stopped = Some("test.hello stopped: it crashed".into());
        assert!(layout(&p, 100).all_text().contains("it crashed -- SPC h l"));
    }

    /// a colour of its own takes its cells from the spans under it.
    #[test]
    fn a_colour_of_its_own_takes_its_cells() {
        use crate::page::Span;
        let spans = vec![Span { line: 0, cols: 0..10, role: Role::Accent }, Span { line: 1, cols: 0..4, role: Role::Bad }];
        let out = colored(spans, [(0, 2..5, (1, 2, 3))].into_iter());
        assert_eq!(out, vec![
            Span { line: 0, cols: 0..2, role: Role::Accent },
            Span { line: 0, cols: 5..10, role: Role::Accent },
            Span { line: 1, cols: 0..4, role: Role::Bad },
            Span { line: 0, cols: 2..5, role: Role::Rgb(1, 2, 3) },
        ]);
    }
}
