//! `SPC f x`: a file as hex and ASCII, sixteen octets a line. View only:
//! nothing here writes to the file. The status line reads the octets at
//! the cursor as numbers and says when a packet or a sync marker starts
//! there; `x` decodes it in the inspector.

use std::path::PathBuf;

use fenix_ccsds::coding::ASM;
use fenix_ccsds::PrimaryHeader;

use crate::page::{fit, fit_tail, frame, Grid, Key, Page, Role};

/// The most of a file the view reads.
pub const LIMIT: u64 = 64 * 1024 * 1024;
/// Lines laid out around the cursor.
const WINDOW: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    /// Decode what starts at this offset.
    Decode(usize),
}

enum Prompt {
    Search(String),
    Goto(String),
}

pub struct HexPage {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    /// The file's size, when only the first `LIMIT` octets were read.
    pub total: Option<u64>,
    cursor: usize,
    prompt: Option<Prompt>,
    pattern: Vec<u8>,
    pending_g: bool,
    pub note: Option<(String, bool)>,
}

impl HexPage {
    pub fn new(path: PathBuf, bytes: Vec<u8>, total: Option<u64>) -> Self {
        HexPage { path, bytes, total, cursor: 0, prompt: None, pattern: Vec::new(), pending_g: false, note: None }
    }

    pub fn typing(&self) -> bool {
        self.prompt.is_some()
    }

    pub fn paste(&mut self, text: &str) {
        match &mut self.prompt {
            Some(Prompt::Search(s)) | Some(Prompt::Goto(s)) => s.push_str(text.trim()),
            None => {}
        }
    }

    fn move_to(&mut self, at: isize) {
        self.cursor = at.clamp(0, self.bytes.len().saturating_sub(1) as isize) as usize;
    }

    fn find(&mut self, forward: bool) {
        if self.pattern.is_empty() {
            return;
        }
        let n = self.pattern.len();
        let hit = if forward {
            self.bytes.get(self.cursor + 1..).and_then(|rest| rest.windows(n).position(|w| w == self.pattern)).map(|i| self.cursor + 1 + i)
        } else {
            self.bytes[..self.cursor.min(self.bytes.len())].windows(n).rposition(|w| w == self.pattern)
        };
        match hit {
            Some(i) => self.cursor = i,
            None => self.note = Some(("no more matches".into(), false)),
        }
    }

    /// A packet header at `at`, when one plausibly starts there.
    fn packet_at(&self, at: usize) -> Option<PrimaryHeader> {
        PrimaryHeader::decode(self.bytes.get(at..)?).filter(|h| h.plausible() && at + h.packet_len() <= self.bytes.len())
    }

    pub fn key(&mut self, key: Key) -> Action {
        self.note = None;
        if let Some(prompt) = &mut self.prompt {
            let text = match prompt {
                Prompt::Search(s) | Prompt::Goto(s) => s,
            };
            match key {
                Key::Escape => self.prompt = None,
                Key::Backspace => {
                    text.pop();
                }
                Key::Char(c) => text.push(c),
                Key::Space => text.push(' '),
                Key::Enter => {
                    let done = self.prompt.take();
                    match done {
                        Some(Prompt::Search(s)) => match fenix_ccsds::hex::parse(&s).or_else(|| (!s.is_empty()).then(|| s.as_bytes().to_vec())) {
                            Some(p) => {
                                self.pattern = p;
                                self.find(true);
                            }
                            None => self.note = Some(("hex octets, or text".into(), true)),
                        },
                        Some(Prompt::Goto(s)) => match fenix_mib::types::parse_int(&s) {
                            Some(n) => self.move_to(n as isize),
                            None => self.note = Some(("an offset: 0x1F40 or 8000".into(), true)),
                        },
                        None => {}
                    }
                }
                _ => {}
            }
            return Action::None;
        }
        let c = self.cursor as isize;
        if std::mem::take(&mut self.pending_g) {
            if key == Key::Char('g') {
                self.cursor = 0;
            }
            return Action::None;
        }
        match key {
            Key::Char('q') | Key::Escape => return Action::Close,
            Key::Right | Key::Char('l') => self.move_to(c + 1),
            Key::Left | Key::Char('h') => self.move_to(c - 1),
            Key::Down | Key::Char('j') => self.move_to(c + 16),
            Key::Up | Key::Char('k') => self.move_to(c - 16),
            Key::Char('d') => self.move_to(c + 16 * 20),
            Key::Char('u') => self.move_to(c - 16 * 20),
            Key::Char('0') => self.move_to(c - c % 16),
            Key::Char('$') => self.move_to(c - c % 16 + 15),
            Key::Char('g') => self.pending_g = true,
            Key::Char('G') => self.move_to(isize::MAX / 2),
            Key::Char('w') => match self.packet_at(self.cursor) {
                Some(h) => self.move_to(c + h.packet_len() as isize),
                None => self.note = Some(("no packet starts here".into(), false)),
            },
            Key::Char(']') => {
                self.pattern = ASM.to_vec();
                self.find(true);
            }
            Key::Char('[') => {
                self.pattern = ASM.to_vec();
                self.find(false);
            }
            Key::Char('/') => self.prompt = Some(Prompt::Search(String::new())),
            Key::Char('n') => self.find(true),
            Key::Char('N') => self.find(false),
            Key::Char('o') => self.prompt = Some(Prompt::Goto(String::new())),
            Key::Char('x') | Key::Enter => return Action::Decode(self.cursor),
            _ => {}
        }
        Action::None
    }

    /// What starts at the cursor, for the status line.
    fn here(&self) -> String {
        let at = self.cursor;
        let b = &self.bytes;
        let mut parts = vec![format!("0x{at:X} · {at}")];
        if let Some(v) = b.get(at) {
            parts.push(format!("u8 {v}"));
        }
        if let Some(v) = b.get(at..at + 2) {
            parts.push(format!("u16 {}", u16::from_be_bytes([v[0], v[1]])));
        }
        if let Some(v) = b.get(at..at + 4) {
            parts.push(format!("u32 {}", u32::from_be_bytes([v[0], v[1], v[2], v[3]])));
        }
        if b.get(at..at + 4) == Some(&ASM[..]) {
            parts.push("sync marker -- x decodes the frame".into());
        } else if let Some(h) = self.packet_at(at) {
            parts.push(format!("{} packet, APID 0x{:03X}, {} octets -- x decodes it, w skips it", if h.is_tc { "TC" } else { "TM" }, h.apid, h.packet_len()));
        }
        parts.join("  ·  ")
    }
}

pub fn title(p: &HexPage) -> String {
    format!("*hex: {}*", p.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
}

pub fn layout(p: &HexPage, cols: usize) -> Page {
    let (left, width) = frame(cols, 100);
    let mut g = Grid::new();
    let size = match p.total {
        Some(t) => format!("first {} of {} octets", LIMIT, t),
        None => format!("{} octets", p.bytes.len()),
    };
    g.put(1, left, &fit(&p.path.display().to_string(), width.saturating_sub(size.len() + 16)), Role::Title);
    g.put(1, (left + width).saturating_sub(size.len() + 12), &format!("{size} · view only"), Role::Muted);
    let status = match &p.prompt {
        Some(Prompt::Search(s)) => format!("/ {s}▏"),
        Some(Prompt::Goto(s)) => format!("go to offset {s}▏"),
        None => p.note.as_ref().map(|(t, _)| t.clone()).unwrap_or_else(|| p.here()),
    };
    let e = g.put(2, left, &fit_tail(&status, width), if p.prompt.is_some() { Role::Title } else if p.note.as_ref().is_some_and(|n| n.1) { Role::Bad } else { Role::Muted });
    if p.prompt.is_some() {
        g.panels.push((2, left..e.max(left + 30)));
    }
    g.rule(3, left..left + width);
    let lines = p.bytes.len().div_ceil(16);
    let cur = p.cursor / 16;
    let start = cur.saturating_sub(WINDOW / 2).min(lines.saturating_sub(WINDOW));
    let mut y = 4;
    if start > 0 {
        g.put(y, left, &format!("↑ 0x{:X}", start * 16), Role::Muted);
        y += 1;
    }
    for line in start..(start + WINDOW).min(lines) {
        let off = line * 16;
        g.put(y, left, &format!("{off:08X}"), Role::Muted);
        for col in 0..16 {
            let i = off + col;
            let Some(b) = p.bytes.get(i) else { break };
            let bx = left + 10 + col * 3 + if col >= 8 { 1 } else { 0 };
            let e = g.put(y, bx, &format!("{b:02X}"), if *b == 0 { Role::Muted } else { Role::Text });
            let c = if b.is_ascii_graphic() { *b as char } else { '·' };
            let ax = left + 10 + 16 * 3 + 2 + col;
            let ae = g.put(y, ax, &c.to_string(), Role::Muted);
            if i == p.cursor {
                g.panels.push((y, bx..e));
                g.panels.push((y, ax..ae));
            }
        }
        if line == cur {
            g.focus(y, left..left + width);
        }
        y += 1;
    }
    g.keys(left, width, &[("h j k l", "move"), ("d u", "page"), ("w", "next packet"), ("] [", "sync markers"), ("/ n N", "find"), ("o", "offset"), ("x", "decode here"), ("q", "close")]);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> HexPage {
        let mut bytes = vec![0u8; 3];
        bytes.extend([0x0B, 0xF2, 0xC1, 0x23, 0x00, 0x01, 0xAA, 0xBB]);
        bytes.extend(ASM);
        bytes.extend(vec![0x41; 30]);
        HexPage::new(PathBuf::from("dump.bin"), bytes, None)
    }

    #[test]
    fn moving_finding_and_reading_what_starts_here() {
        let mut p = page();
        let text = layout(&p, 120).text;
        assert!(text.contains("00000000  00 00 00 0B F2 C1 23 00  01 AA BB 1A CF FC 1D 41"), "{text}");
        p.key(Key::Char('l'));
        p.key(Key::Char('l'));
        p.key(Key::Char('l'));
        assert!(layout(&p, 120).text.contains("TM packet, APID 0x3F2, 8 octets"));
        p.key(Key::Char('w'));
        assert!(layout(&p, 120).text.contains("sync marker"));
        assert_eq!(p.key(Key::Char('x')), Action::Decode(11));
        p.key(Key::Char('o'));
        for c in "0x3".chars() {
            p.key(Key::Char(c));
        }
        p.key(Key::Enter);
        assert_eq!(p.cursor, 3);
        p.key(Key::Char('/'));
        for c in "41 41".chars() {
            p.key(if c == ' ' { Key::Space } else { Key::Char(c) });
        }
        p.key(Key::Enter);
        assert_eq!(p.cursor, 15);
        p.key(Key::Char('n'));
        assert_eq!(p.cursor, 16);
    }
}
