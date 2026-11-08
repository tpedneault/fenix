//! A terminal's screen as Fenix draws it: each cell's colour in the
//! theme's own ANSI palette (`Theme::ansi`), and its look -- bold, dim,
//! italic, underlined, inverse -- as the text and the rects behind and
//! under it. Kept apart from `redraw` so it can be tested on a screen a
//! test feeds, without a GPU or a shell.
//!
//! Inverse matters more than it looks: PowerShell's menu completion
//! (Ctrl-Space), `less`'s status line, `fzf`'s and `htop`'s selected rows
//! are all drawn by swapping a cell's colours, and a terminal that drops
//! the swap shows the menu with nothing selected in it.

use crate::theme::{mix, Theme};

/// What a span of terminal text is drawn in besides its colour.
pub(crate) const BOLD: u8 = 1;
pub(crate) const ITALIC: u8 = 2;

/// A terminal's screen, laid out for drawing.
#[derive(Debug, Default)]
pub(crate) struct TermView {
    /// The text, a run of one colour and look a span, rows ended by "\n".
    pub spans: Vec<(String, glyphon::Color, bool)>,
    /// Which spans are bold or italic: (span, `BOLD` | `ITALIC`).
    pub styles: Vec<(usize, u8)>,
    /// Cells on a background of their own: (row, first column, end, colour).
    pub backgrounds: Vec<(usize, usize, usize, [f32; 4])>,
    /// Cells underlined: (row, first column, end, colour).
    pub underlines: Vec<(usize, usize, usize, [f32; 4])>,
}

/// A colour a cell names, in `theme`: the default as given, one of the 16
/// ANSI colours from the theme, the rest of the 256 xterm's, or exact.
fn resolve(theme: &Theme, color: vt100::Color, default: [f32; 4]) -> [f32; 4] {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Idx(i @ 0..=15) => theme.ansi[i as usize],
        vt100::Color::Idx(i) => {
            let (r, g, b) = super::xterm_256_rgb(i);
            [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
        }
        vt100::Color::Rgb(r, g, b) => [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0],
    }
}

fn text_color(c: [f32; 4]) -> glyphon::Color {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    glyphon::Color::rgb(byte(c[0]), byte(c[1]), byte(c[2]))
}

/// One cell's colours and look: its text colour, its own background if
/// it has one, and whether it's bold, italic, underlined.
fn cell_look(theme: &Theme, cell: &vt100::Cell) -> ([f32; 4], Option<[f32; 4]>, u8, bool) {
    let default_fg = crate::theme::rgba_of(theme.fg);
    let mut fg_color = cell.fgcolor();
    // Bold in one of the first eight colours is its bright form, as
    // terminals have always drawn it -- `ls`'s bold blue directories,
    // PowerShell's bold errors.
    if cell.bold() {
        if let vt100::Color::Idx(i @ 0..=7) = fg_color {
            fg_color = vt100::Color::Idx(i + 8);
        }
    }
    let mut fg = resolve(theme, fg_color, default_fg);
    let mut bg = match cell.bgcolor() {
        vt100::Color::Default => None,
        other => Some(resolve(theme, other, theme.bg)),
    };
    if cell.inverse() {
        let was_fg = fg;
        fg = bg.unwrap_or(theme.bg);
        bg = Some(was_fg);
    }
    // Dim: halfway to what's behind it. PSReadLine's suggestions from
    // history are drawn dim (and italic), after what you've typed.
    if cell.dim() {
        fg = mix(fg, bg.unwrap_or(theme.bg), 0.45);
    }
    let style = if cell.bold() { BOLD } else { 0 } | if cell.italic() { ITALIC } else { 0 };
    (fg, bg, style, cell.underline())
}

/// `screen` as it's drawn in `theme`.
pub(crate) fn view(theme: &Theme, screen: &vt100::Screen) -> TermView {
    let (rows, cols) = screen.size();
    let mut out = TermView::default();
    for row in 0..rows {
        // The run being gathered: its text, colour and look.
        let mut run: Option<(String, [f32; 4], u8)> = None;
        let mut bg_run: Option<(usize, [f32; 4])> = None;
        let mut under_run: Option<(usize, [f32; 4])> = None;
        for col in 0..cols {
            let cell = screen.cell(row, col);
            let (fg, bg, style, underline) = cell.map(|c| cell_look(theme, c)).unwrap_or((crate::theme::rgba_of(theme.fg), None, 0, false));
            let text = cell.map(|c| c.contents()).filter(|t| !t.is_empty()).unwrap_or(" ");
            match &mut run {
                Some((t, c, s)) if *c == fg && *s == style => t.push_str(text),
                _ => {
                    if let Some((t, c, s)) = run.take() {
                        flush(&mut out, t, c, s);
                    }
                    run = Some((text.to_string(), fg, style));
                }
            }
            let col = col as usize;
            match (bg, bg_run) {
                (Some(b), Some((_, c))) if b == c => {}
                (b, prev) => {
                    if let Some((start, c)) = prev {
                        out.backgrounds.push((row as usize, start, col, c));
                    }
                    bg_run = b.map(|b| (col, b));
                }
            }
            let under = underline.then_some(fg);
            match (under, under_run) {
                (Some(u), Some((_, c))) if u == c => {}
                (u, prev) => {
                    if let Some((start, c)) = prev {
                        out.underlines.push((row as usize, start, col, c));
                    }
                    under_run = u.map(|u| (col, u));
                }
            }
        }
        if let Some((t, c, s)) = run.take() {
            flush(&mut out, t, c, s);
        }
        if let Some((start, c)) = bg_run {
            out.backgrounds.push((row as usize, start, cols as usize, c));
        }
        if let Some((start, c)) = under_run {
            out.underlines.push((row as usize, start, cols as usize, c));
        }
        if row + 1 < rows {
            out.spans.push(("\n".to_string(), theme.fg, false));
        }
    }
    out
}

fn flush(out: &mut TermView, text: String, color: [f32; 4], style: u8) {
    if style != 0 {
        out.styles.push((out.spans.len(), style));
    }
    out.spans.push((text, text_color(color), false));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(bytes: &[u8]) -> vt100::Parser {
        let mut p = vt100::Parser::new(2, 20, 0);
        p.process(bytes);
        p
    }

    fn theme() -> &'static Theme {
        crate::theme::default_theme()
    }

    /// The first 16 colours are the theme's; bold makes one of the first
    /// eight its bright form, and is drawn bold too.
    #[test]
    fn ansi_colours_are_the_themes_and_bold_brightens() {
        let p = screen(b"\x1b[31mred\x1b[0m \x1b[1;34mdir\x1b[0m");
        let v = view(theme(), p.screen());
        let red = v.spans.iter().position(|(t, _, _)| t == "red").unwrap();
        assert_eq!(v.spans[red].1, text_color(theme().ansi[1]));
        let dir = v.spans.iter().position(|(t, _, _)| t == "dir").unwrap();
        assert_eq!(v.spans[dir].1, text_color(theme().ansi[12]), "bold blue is bright blue");
        assert!(v.styles.contains(&(dir, BOLD)));
    }

    /// Black text reads on a dark page: it's the theme's black, a grey.
    #[test]
    fn black_text_reads_on_a_dark_page() {
        let p = screen(b"\x1b[30mansi0\x1b[0m");
        let v = view(theme(), p.screen());
        let span = v.spans.iter().find(|(t, _, _)| t.starts_with("ansi0")).expect("its own span");
        let c = crate::theme::rgba_of(span.1);
        assert!(crate::theme::contrast_of(c, theme().bg) >= 2.5, "{:?}", span.1);
    }

    /// Inverse swaps a cell's colours: what a menu's selected item is.
    #[test]
    fn inverse_swaps_the_colours() {
        let p = screen(b"ab\x1b[7mcd\x1b[0m");
        let v = view(theme(), p.screen());
        let cd = v.spans.iter().position(|(t, _, _)| t == "cd").unwrap();
        assert_eq!(v.spans[cd].1, text_color(theme().bg), "the page's colour as the text");
        assert_eq!(v.backgrounds, [(0, 2, 4, crate::theme::rgba_of(theme().fg))], "the text's colour behind it");
    }

    /// Dim fades towards the background, italic and underline are kept:
    /// PSReadLine's suggestion from history is dim and italic.
    #[test]
    fn dim_italic_and_underline() {
        let p = screen(b"ls\x1b[2;3m -la\x1b[0m \x1b[4mlink\x1b[0m");
        let v = view(theme(), p.screen());
        let hint = v.spans.iter().position(|(t, _, _)| t == " -la").unwrap();
        assert_ne!(v.spans[hint].1, theme().fg, "dimmed");
        assert!(v.styles.contains(&(hint, ITALIC)));
        assert_eq!(v.underlines.len(), 1);
        assert_eq!((v.underlines[0].1, v.underlines[0].2), (7, 11), "\"link\", after \"ls -la \"");
    }

    /// A row of plain text is one span, ended by a line break.
    #[test]
    fn plain_rows_are_one_span_each() {
        let p = screen(b"hello\r\nworld");
        let v = view(theme(), p.screen());
        let text: String = v.spans.iter().map(|(t, _, _)| t.as_str()).collect();
        assert_eq!(text.lines().map(str::trim_end).collect::<Vec<_>>(), ["hello", "world"]);
        assert!(v.styles.is_empty() && v.backgrounds.is_empty() && v.underlines.is_empty());
    }
}
