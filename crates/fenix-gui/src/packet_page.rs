//! `SPC k d`: the packet inspector. Bytes -- from the cursor, a
//! selection, the clipboard, a recording's row -- decoded layer by layer
//! into a tree of fields (the host does the decoding: `fenix-ccsds` and,
//! with a MIB, `fenix_mib::packets`), drawn beside the bytes, each byte
//! coloured by the layer it belongs to and the selected field's bytes
//! outlined. `Enter` on a field that names something opens it: a MIB
//! definition, or the standard that defines the field. `/` finds fields
//! by name, raw bytes, value or check -- unfolding what hides them -- and
//! `n`/`N` step through what it found.

use std::collections::HashSet;

use fenix_ccsds::field::{Check, Field, Link};

use crate::mib_page::MibKey;
use crate::page::{fit, frame, Filter, Grid, Key, Page, Popup, Role};

/// What the bytes are read as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeAs {
    Auto,
    Packet,
    Frame,
    Cltu,
    Cfdp,
    Clcw,
    Time,
}

impl DecodeAs {
    pub const ALL: [DecodeAs; 7] = [DecodeAs::Auto, DecodeAs::Packet, DecodeAs::Frame, DecodeAs::Cltu, DecodeAs::Cfdp, DecodeAs::Clcw, DecodeAs::Time];

    pub fn label(self) -> &'static str {
        match self {
            DecodeAs::Auto => "whatever it looks like",
            DecodeAs::Packet => "a space packet",
            DecodeAs::Frame => "a transfer frame (the project's frames)",
            DecodeAs::Cltu => "a CLTU",
            DecodeAs::Cfdp => "a CFDP PDU",
            DecodeAs::Clcw => "a CLCW",
            DecodeAs::Time => "a time code (the project's TM time)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    Follow(Link),
    /// Decode again: the reading changed.
    Redecode,
    Copy(String, &'static str),
    /// The CCSDS settings.
    Settings,
}

pub struct PacketPage {
    pub key: MibKey,
    pub bytes: Vec<u8>,
    /// Where the bytes came from.
    pub source: String,
    pub reading: DecodeAs,
    /// Set by the host.
    pub root: Field,
    sel: usize,
    folded: HashSet<usize>,
    menu: Option<usize>,
    pending_z: bool,
    help: bool,
    pub note: Option<(String, bool)>,
    /// `/`: fields with these words.
    pub filter: Filter,
}

/// One visible row: its index in the tree's walk, depth, field.
fn rows(root: &Field, folded: &HashSet<usize>) -> Vec<(usize, usize, Field)> {
    let mut out = Vec::new();
    let mut skip_below: Option<usize> = None;
    for (i, (depth, f)) in root.walk().into_iter().enumerate() {
        if let Some(d) = skip_below {
            if depth > d {
                continue;
            }
            skip_below = None;
        }
        if folded.contains(&i) {
            skip_below = Some(depth);
        }
        out.push((i, depth, f.clone()));
    }
    out
}

/// Whether `f` has every word of `filter` in its name, raw bytes, value
/// or check. A group by its name only: its value sums up the fields in
/// it, which are found themselves.
fn field_matches(f: &Field, filter: &Filter) -> bool {
    if filter.is_empty() {
        return false;
    }
    if !f.children.is_empty() {
        return filter.matches(&f.name);
    }
    let check = match &f.check {
        Some(Check::Ok(m) | Check::Warn(m) | Check::Bad(m)) => m.as_str(),
        None => "",
    };
    filter.matches(&format!("{} {} {} {check}", f.name, f.raw, f.value))
}

/// A tree as plain text, for the clipboard.
pub fn tree_text(root: &Field) -> String {
    root.walk()
        .into_iter()
        .map(|(d, f)| {
            let check = match &f.check {
                Some(Check::Ok(_)) | None => String::new(),
                Some(Check::Warn(m)) => format!("  (warning: {m})"),
                Some(Check::Bad(m)) => format!("  (bad: {m})"),
            };
            format!("{}{}  {}  {}{check}", "  ".repeat(d), f.name, f.raw, f.value).trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl PacketPage {
    pub fn new(key: MibKey, bytes: Vec<u8>, source: String) -> Self {
        PacketPage { key, bytes, source, reading: DecodeAs::Auto, root: Field::default(), sel: 0, folded: HashSet::new(), menu: None, pending_z: false, help: false, note: None, filter: Filter::default() }
    }

    pub fn typing(&self) -> bool {
        self.filter.typing
    }

    pub fn paste(&mut self, text: &str) {
        self.filter.paste(text);
        self.find(0, true);
    }

    /// The fields the filter finds, by their place in the tree's walk.
    fn found(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return Vec::new();
        }
        self.root.walk().into_iter().enumerate().filter(|(_, (_, f))| field_matches(f, &self.filter)).map(|(i, _)| i).collect()
    }

    /// The walk index of the selected row.
    fn sel_index(&self) -> Option<usize> {
        let r = rows(&self.root, &self.folded);
        r.get(self.sel.min(r.len().saturating_sub(1))).map(|(i, _, _)| *i)
    }

    /// Selects field `i` of the walk, unfolding whatever it's inside.
    fn reveal(&mut self, i: usize) {
        let walk = self.root.walk();
        let mut depth = walk.get(i).map(|(d, _)| *d).unwrap_or(0);
        for j in (0..i).rev() {
            if depth == 0 {
                break;
            }
            if walk[j].0 < depth {
                self.folded.remove(&j);
                depth = walk[j].0;
            }
        }
        if let Some(at) = rows(&self.root, &self.folded).iter().position(|(k, _, _)| *k == i) {
            self.sel = at;
        }
    }

    /// Goes to the next field found from the selection (`skip`: past it)
    /// -- or the previous, going back -- wrapping at the end.
    fn find(&mut self, skip: usize, forward: bool) {
        let found = self.found();
        if found.is_empty() {
            if !self.filter.is_empty() {
                self.note = Some(("no field has that".into(), true));
            }
            return;
        }
        let here = self.sel_index().unwrap_or(0);
        let next = if forward {
            found.iter().copied().find(|&i| i >= here + skip).or_else(|| found.first().copied())
        } else {
            found.iter().rev().copied().find(|&i| i < here).or_else(|| found.last().copied())
        };
        if let Some(i) = next {
            self.reveal(i);
        }
    }

    fn selected(&self) -> Option<Field> {
        let r = rows(&self.root, &self.folded);
        r.get(self.sel.min(r.len().saturating_sub(1))).map(|(_, _, f)| f.clone())
    }

    pub fn key(&mut self, key: Key) -> Action {
        self.note = None;
        if self.help {
            self.help = false;
            return Action::None;
        }
        if let Some(at) = self.menu {
            match key {
                Key::Escape | Key::Char('q') => self.menu = None,
                Key::Down | Key::Char('j') => self.menu = Some((at + 1).min(DecodeAs::ALL.len() - 1)),
                Key::Up | Key::Char('k') => self.menu = Some(at.saturating_sub(1)),
                Key::Enter => {
                    self.reading = DecodeAs::ALL[at];
                    self.menu = None;
                    self.sel = 0;
                    self.folded.clear();
                    return Action::Redecode;
                }
                _ => {}
            }
            return Action::None;
        }
        if self.filter.typing {
            if self.filter.key(key) {
                self.find(0, true);
            }
            return Action::None;
        }
        let r = rows(&self.root, &self.folded);
        let n = r.len();
        if std::mem::take(&mut self.pending_z) {
            if key == Key::Char('a') {
                self.toggle_fold(&r);
            }
            return Action::None;
        }
        match key {
            Key::Escape if !self.filter.is_empty() => self.filter.clear(),
            Key::Char('q') | Key::Escape => return Action::Close,
            Key::Char('?') => self.help = true,
            Key::Char('/') => self.filter.start(),
            Key::Char('n') => self.find(1, true),
            Key::Char('N') => self.find(0, false),
            Key::Down | Key::Char('j') => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            Key::Up | Key::Char('k') => self.sel = self.sel.saturating_sub(1),
            Key::Char('g') => self.sel = 0,
            Key::Char('G') => self.sel = n.saturating_sub(1),
            Key::Char('z') => self.pending_z = true,
            Key::Space | Key::Tab => self.toggle_fold(&r),
            Key::Enter => {
                if let Some(link) = self.selected().and_then(|f| f.link) {
                    return Action::Follow(link);
                }
                self.toggle_fold(&r);
            }
            Key::Char('y') => {
                if let Some(f) = self.selected() {
                    let v = if f.value.is_empty() { f.raw } else { f.value };
                    return Action::Copy(v, "value");
                }
            }
            Key::Char('Y') => return Action::Copy(tree_text(&self.root), "decode"),
            Key::Char('b') => return Action::Copy(fenix_ccsds::field::hex(&self.bytes), "bytes"),
            Key::Char('a') => self.menu = Some(DecodeAs::ALL.iter().position(|d| *d == self.reading).unwrap_or(0)),
            Key::Char('p') => return Action::Settings,
            _ => {}
        }
        Action::None
    }

    fn toggle_fold(&mut self, r: &[(usize, usize, Field)]) {
        if let Some((i, _, f)) = r.get(self.sel) {
            if !f.children.is_empty() && !self.folded.remove(i) {
                self.folded.insert(*i);
            }
        }
    }
}

/// The role each layer's bytes are drawn in, by top-level field.
const LAYER_ROLES: [Role; 5] = [Role::Accent, Role::Good, Role::Warn, Role::Title, Role::Muted];

fn mark(check: &Option<Check>) -> (&'static str, Role, String) {
    match check {
        Some(Check::Ok(m)) => ("✓", Role::Good, m.clone()),
        Some(Check::Warn(m)) => ("⚠", Role::Warn, m.clone()),
        Some(Check::Bad(m)) => ("✗", Role::Bad, m.clone()),
        None => ("", Role::Muted, String::new()),
    }
}

pub fn title(p: &PacketPage) -> String {
    format!("*decode: {}*", fit(&p.root.name, 30))
}

pub fn layout(p: &PacketPage, cols: usize) -> Page {
    let (left, width) = frame(cols, 190);
    let mut g = Grid::new();
    g.put(1, left, "Decode ›", Role::Muted);
    let right = format!("{} bytes · {}", p.bytes.len(), p.source);
    g.put(1, (left + width).saturating_sub(right.chars().count()), &fit(&right, width / 2), Role::Muted);
    let mut x = g.put(2, left, &p.root.name, Role::Title) + 2;
    if !p.root.value.is_empty() {
        x = g.put(2, x, &p.root.value, Role::Text) + 2;
    }
    let walk = p.root.walk();
    let bad = walk.iter().filter(|(_, f)| matches!(f.check, Some(Check::Bad(_)))).count();
    let warn = walk.iter().filter(|(_, f)| matches!(f.check, Some(Check::Warn(_)))).count();
    let (summary, role) = match (bad, warn) {
        (0, 0) => ("✓ every check passes".to_string(), Role::Good),
        (0, w) => (format!("⚠ {w} warning{}", if w == 1 { "" } else { "s" }), Role::Warn),
        (b, _) => (format!("✗ {b} failed · {warn} warnings"), Role::Bad),
    };
    g.put(2, x, &summary, role);
    g.put(3, left, &format!("read as {}  ·  a changes it", p.reading.label()), Role::Muted);
    if let Some((text, bad)) = &p.note {
        g.put(3, left + 60, &fit(text, width.saturating_sub(60)), if *bad { Role::Bad } else { Role::Good });
    }
    let found = p.found();
    let (search, role) = p.filter.line("find a field: its name, raw bytes, value or check", width / 2);
    let e = g.put(4, left, &search, role);
    if p.filter.typing {
        g.panels.push((4, left..e.max(left + 30)));
    }
    if !p.filter.is_empty() {
        let count = match found.iter().position(|&i| Some(i) == p.sel_index()) {
            Some(k) => format!("{} of {} · n N", k + 1, found.len()),
            None => format!("{} found · n N", found.len()),
        };
        g.put(4, e + 2, &count, if found.is_empty() { Role::Bad } else { Role::Muted });
    }
    g.rule(5, left..left + width);

    let r = rows(&p.root, &p.folded);
    let sel = p.sel.min(r.len().saturating_sub(1));
    let selected = r.get(sel).map(|(_, _, f)| (f.bit, f.bit + f.bits));

    // The bytes.
    let top = 6;
    let hex_w = 6 + 16 * 3 + 1 + 16;
    let side = width >= hex_w + 60;
    let layers: Vec<(usize, usize)> = p.root.children.iter().map(|c| (c.bit, c.bit + c.bits)).collect();
    // 64 rows of 16 at most: past that (a CADU), the rows around the
    // selected field.
    let all_rows = p.bytes.len().div_ceil(16);
    let rows_of_bytes = all_rows.min(64);
    let sel_row = selected.map(|(s, _)| s / 8 / 16).unwrap_or(0);
    let first = if sel_row < rows_of_bytes { 0 } else { sel_row.saturating_sub(rows_of_bytes / 2).min(all_rows - rows_of_bytes) };
    for i in 0..rows_of_bytes {
        let row = first + i;
        let y = top + i;
        g.put(y, left, &format!("{:04X}", row * 16), Role::Muted);
        for col in 0..16 {
            let i = row * 16 + col;
            let Some(b) = p.bytes.get(i) else { break };
            let bit = i * 8;
            let layer = layers.iter().position(|(s, e)| bit >= *s && bit < *e);
            let role = layer.map(|l| LAYER_ROLES[l % LAYER_ROLES.len()]).unwrap_or(Role::Muted);
            let bx = left + 6 + col * 3;
            let end = g.put(y, bx, &format!("{b:02X}"), role);
            if selected.is_some_and(|(s, e)| e > s && bit < e && bit + 8 > s) {
                g.panels.push((y, bx..end));
            }
            let c = if b.is_ascii_graphic() { *b as char } else { '·' };
            g.put(y, left + 6 + 16 * 3 + 1 + col, &c.to_string(), Role::Muted);
        }
    }
    if all_rows > rows_of_bytes {
        let last = ((first + rows_of_bytes) * 16).min(p.bytes.len()) - 1;
        g.put(top + rows_of_bytes, left, &format!("octets 0x{:X}-0x{last:X} of {} -- the rows follow the field", first * 16, p.bytes.len()), Role::Muted);
    }

    // The tree.
    let (tx, mut y, tw) = if side { (left + hex_w + 4, top, width - hex_w - 4) } else { (left, top + rows_of_bytes + 2, width) };
    let (raw_x, val_x) = (tx + tw * 2 / 5, tx + tw * 3 / 5);
    g.put(y, tx, "FIELD", Role::Muted);
    g.put(y, raw_x, "RAW", Role::Muted);
    g.put(y, val_x, "VALUE", Role::Muted);
    y += 1;
    let mut anchor = y;
    for (k, (i, depth, f)) in r.iter().enumerate() {
        let fold = if f.children.is_empty() { "  " } else if p.folded.contains(i) { "▸ " } else { "▾ " };
        let name_role = if f.link.is_some() { Role::Accent } else if *depth == 0 || !f.children.is_empty() { Role::Title } else { Role::Text };
        let nx = tx + depth * 2;
        let e = g.put(y, nx, fold, Role::Muted);
        let ne = g.put(y, e, &fit(&f.name, raw_x.saturating_sub(e + 1)), name_role);
        if found.binary_search(i).is_ok() {
            g.panels.push((y, e..ne.max(e + 1)));
        }
        g.put(y, raw_x, &fit(&f.raw, val_x.saturating_sub(raw_x + 1)), Role::Muted);
        let (m, role, msg) = mark(&f.check);
        let value = if msg.is_empty() || matches!(f.check, Some(Check::Ok(_))) { f.value.clone() } else { format!("{} -- {msg}", f.value) };
        let vx = g.put(y, val_x, &fit(&value, (tx + tw).saturating_sub(val_x + 2)), if matches!(f.check, Some(Check::Bad(_))) { Role::Bad } else { Role::Text });
        if !m.is_empty() {
            g.put(y, vx + 1, m, role);
        }
        if k == sel {
            g.focus(y, tx..tx + tw);
            anchor = y;
        }
        y += 1;
    }

    if let Some(at) = p.menu {
        let mut rows = vec![vec![("Read the bytes as".to_string(), Role::Title)], Vec::new()];
        for (i, d) in DecodeAs::ALL.iter().enumerate() {
            let on = i == at;
            rows.push(vec![(format!("{}{}", if on { "› " } else { "  " }, d.label()), if on { Role::Title } else { Role::Text })]);
        }
        rows.push(Vec::new());
        rows.push(vec![("Enter picks · Esc leaves it".into(), Role::Muted)]);
        g.popup = Some(Popup { line: anchor, col: tx + 4, rows });
    } else if p.help {
        let rows: Vec<Vec<(String, Role)>> = [
            ("j k g G", "fields"),
            ("Enter", "open what the field names; else fold"),
            ("Space za", "fold, unfold"),
            ("/ n N", "find fields by name, bytes, value; next, previous"),
            ("y Y b", "copy the value, the whole decode, the bytes"),
            ("a", "read the bytes as something else"),
            ("p", "the project's CCSDS settings"),
            ("q", "close"),
        ]
        .iter()
        .map(|(k, w)| vec![(format!("{k:<10}"), Role::Accent), (w.to_string(), Role::Text)])
        .collect();
        let mut all = vec![vec![("Keys".to_string(), Role::Title)], Vec::new()];
        all.extend(rows);
        g.popup = Some(Popup { line: anchor, col: tx + 4, rows: all });
    }
    if p.filter.typing {
        g.keys(left, width, &[("Enter", "keep"), ("Esc", "clear")]);
    } else {
        g.keys(left, width, &[("j k", "fields"), ("Enter", "open"), ("za", "fold"), ("/", "find"), ("n N", "next, previous"), ("y", "copy value"), ("Y", "copy all"), ("b", "copy bytes"), ("a", "read as"), ("?", "all keys"), ("q", "close")]);
    }
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TM: [u8; 29] = [
        0x0B, 0xF2, 0xC1, 0x23, 0x00, 0x16, 0x20, 0x03, 0x19, 0x00, 0x42, 0x00, 0x00, 0x81, 0x4B, 0x87, 0x8A, 0x80, 0x00, 0x00, 0x01, 0x01, 0x0B, 0xB8, 0x0A, 0x28,
        0x02, 0x60, 0xE5,
    ];

    fn page() -> PacketPage {
        let mut p = PacketPage::new(MibKey::default(), TM.to_vec(), "test".into());
        p.root = fenix_ccsds::pus::decode(&TM, &fenix_ccsds::Profile::default()).unwrap().2;
        p
    }

    #[test]
    fn the_bytes_and_the_tree_are_laid_out_side_by_side() {
        let p = page();
        let text = layout(&p, 200).text;
        assert!(text.contains("0000  0B F2 C1 23 00 16 20 03"), "{text}");
        assert!(text.contains("TM(3,25)") && text.contains("✓ every check passes"), "{text}");
        assert!(text.contains("APID") && text.contains("0x3F2"), "{text}");
        assert!(text.contains("2026-09-27 14:32:05.500 UTC"), "{text}");
    }

    #[test]
    fn past_64_rows_the_hex_follows_the_selected_field() {
        let bytes: Vec<u8> = (0..1279u32).map(|i| i as u8).collect();
        let mut p = PacketPage::new(MibKey::default(), bytes, "test".into());
        let near_end = Field::new("CLCW", 1111 * 8, 32, "", "");
        p.root = Field::group("CADU", vec![Field::new("sync marker", 0, 32, "", ""), near_end]);
        let text = layout(&p, 200).text;
        assert!(text.contains("0000  00 01 02") && text.contains("octets 0x0-0x3FF of 1279"), "{text}");
        p.key(Key::Char('j'));
        p.key(Key::Char('j'));
        let text = layout(&p, 200).text;
        assert!(text.contains("0450  50 51 52") && !text.contains("0000  00 01"), "the CLCW's row is shown: {text}");
    }

    #[test]
    fn slash_finds_fields_unfolding_them_and_n_steps_through() {
        let mut p = page();
        // Fold the whole packet: what's found is unfolded to.
        p.key(Key::Space);
        assert_eq!(rows(&p.root, &p.folded).len(), 1);
        p.key(Key::Char('/'));
        p.paste("apid");
        p.key(Key::Enter);
        let f = p.selected().unwrap();
        assert_eq!(f.name, "APID", "the first match, unfolded to");
        assert!(rows(&p.root, &p.folded).len() > 1);
        let text = layout(&p, 200).text;
        assert!(text.contains("/ apid") && text.contains("1 of"), "{text}");
        let before = p.sel;
        p.key(Key::Char('n'));
        p.key(Key::Char('N'));
        assert_eq!(p.sel, before, "n then N comes back");
        p.key(Key::Escape);
        assert!(p.filter.is_empty());
        p.key(Key::Char('/'));
        p.paste("no-such-field");
        assert!(p.note.as_ref().is_some_and(|n| n.1), "says nothing has it");
    }

    #[test]
    fn folding_links_copying_and_reading_as() {
        let mut p = page();
        let before = rows(&p.root, &p.folded).len();
        p.key(Key::Char('j'));
        p.key(Key::Space);
        assert!(rows(&p.root, &p.folded).len() < before, "the primary header folds");
        p.key(Key::Char('j'));
        p.key(Key::Char('j'));
        p.key(Key::Char('j'));
        p.key(Key::Char('j'));
        let Action::Follow(Link::Standard(std, _)) = (loop {
            let a = p.key(Key::Enter);
            if matches!(a, Action::Follow(_)) {
                break a;
            }
            p.key(Key::Char('j'));
        }) else { panic!() };
        assert!(std.starts_with("ECSS") || std.starts_with("CCSDS"));
        assert!(matches!(p.key(Key::Char('b')), Action::Copy(ref s, "bytes") if s.starts_with("0B F2")));
        p.key(Key::Char('a'));
        p.key(Key::Char('j'));
        assert_eq!(p.key(Key::Enter), Action::Redecode);
        assert_eq!(p.reading, DecodeAs::Packet);
        assert!(tree_text(&p.root).contains("  APID  0x3F2  1010"));
    }
}
