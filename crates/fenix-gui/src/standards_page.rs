//! `SPC k ?`: the standards library. The CCSDS and ECSS standards the
//! mission tools refer to, each with the PDF of it found in the folder
//! `ccsds.library` names (by the CCSDS file naming, `133x0b2e2.pdf` for
//! 133.0-B-2, or the ECSS number in the name). Fenix doesn't ship them:
//! CCSDS publishes its blue books freely, ECSS to registered users.

use std::path::{Path, PathBuf};

use crate::page::{fit, frame, Filter, Grid, Key, Page, Role};

/// A standard: the id fields link with, its title, and how its file is
/// recognized (a CCSDS file-name prefix, or text in an ECSS file's name).
pub struct Standard {
    pub id: &'static str,
    pub title: &'static str,
    pub file: &'static str,
    pub url: &'static str,
}

const CCSDS: &str = "https://public.ccsds.org/Publications/BlueBooks.aspx";
const ECSS: &str = "https://ecss.nl/standards/";

pub const STANDARDS: &[Standard] = &[
    Standard { id: "CCSDS 133.0-B", title: "Space Packet Protocol", file: "133x0b", url: CCSDS },
    Standard { id: "CCSDS 132.0-B", title: "TM Space Data Link Protocol", file: "132x0b", url: CCSDS },
    Standard { id: "CCSDS 232.0-B", title: "TC Space Data Link Protocol", file: "232x0b", url: CCSDS },
    Standard { id: "CCSDS 732.0-B", title: "AOS Space Data Link Protocol", file: "732x0b", url: CCSDS },
    Standard { id: "CCSDS 732.1-B", title: "Unified Space Data Link Protocol", file: "732x1b", url: CCSDS },
    Standard { id: "CCSDS 131.0-B", title: "TM Synchronization and Channel Coding", file: "131x0b", url: CCSDS },
    Standard { id: "CCSDS 231.0-B", title: "TC Synchronization and Channel Coding", file: "231x0b", url: CCSDS },
    Standard { id: "CCSDS 301.0-B", title: "Time Code Formats", file: "301x0b", url: CCSDS },
    Standard { id: "CCSDS 727.0-B", title: "CCSDS File Delivery Protocol (CFDP)", file: "727x0b", url: CCSDS },
    Standard { id: "CCSDS 660.0-B", title: "XML Telemetric and Command Exchange (XTCE)", file: "660x0b", url: CCSDS },
    Standard { id: "CCSDS 355.0-B", title: "Space Data Link Security Protocol", file: "355x0b", url: CCSDS },
    Standard { id: "ECSS-E-ST-70-41C", title: "Telemetry and telecommand packet utilization", file: "70-41c", url: ECSS },
    Standard { id: "ECSS-E-70-41A", title: "Ground systems and operations: telemetry and telecommand packet utilization", file: "70-41a", url: ECSS },
];

/// The PDFs in `folder` (and the folders just inside it).
pub fn pdfs_in(folder: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![folder.to_path_buf()];
    if let Ok(entries) = std::fs::read_dir(folder) {
        dirs.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    }
    for d in dirs {
        if let Ok(entries) = std::fs::read_dir(&d) {
            out.extend(entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))));
        }
    }
    out.sort();
    out
}

/// The file for `std` among `pdfs`: the newest issue when there are
/// several (the names sort by issue).
pub fn file_for<'a>(std: &Standard, pdfs: &'a [PathBuf]) -> Option<&'a PathBuf> {
    pdfs.iter()
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            if std.id.starts_with("CCSDS") { name.starts_with(std.file) } else { name.contains(std.file) }
        })
        .max()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    Open(PathBuf),
    Url(&'static str),
    Settings,
    /// Search every standard the folder has for this text.
    Search(String),
    /// A standard's PDF at a page (from 0).
    OpenAt(PathBuf, u32),
}

/// One place a search found: which standard, its page, the words around.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub standard: usize,
    pub page: u32,
    pub context: String,
}

/// Enough to go through; a search past it keeps the first.
const MAX_HITS: usize = 500;

pub struct StandardsPage {
    pub folder: Option<PathBuf>,
    pub pdfs: Vec<PathBuf>,
    sel: usize,
    /// `/`: what to look for across the standards.
    pub query: Filter,
    /// The search last run, what it found, and how many standards it's
    /// still reading.
    pub searched: Option<String>,
    pub hits: Vec<Hit>,
    pub reading: usize,
    hit: usize,
}

impl StandardsPage {
    pub fn new(folder: Option<PathBuf>) -> Self {
        let pdfs = folder.as_deref().map(pdfs_in).unwrap_or_default();
        StandardsPage { folder, pdfs, sel: 0, query: Filter::default(), searched: None, hits: Vec::new(), reading: 0, hit: 0 }
    }

    pub fn typing(&self) -> bool {
        self.query.typing
    }

    /// The standards a search can read: (index, file).
    pub fn searchable(&self) -> Vec<(usize, PathBuf)> {
        STANDARDS.iter().enumerate().filter_map(|(i, s)| Some((i, file_for(s, &self.pdfs)?.clone()))).collect()
    }

    /// A search starts over `reading` standards.
    pub fn search_started(&mut self, query: &str, reading: usize) {
        self.searched = Some(query.to_string());
        self.hits.clear();
        self.reading = reading;
        self.hit = 0;
    }

    /// What one standard's search found; `done` when it's read to the end.
    pub fn found(&mut self, standard: usize, hits: impl IntoIterator<Item = (u32, String)>, done: bool) {
        let room = MAX_HITS.saturating_sub(self.hits.len());
        self.hits.extend(hits.into_iter().take(room).map(|(page, context)| Hit { standard, page, context }));
        // In the standards' order, then by page.
        self.hits.sort_by_key(|h| (h.standard, h.page));
        if done {
            self.reading = self.reading.saturating_sub(1);
        }
    }

    pub fn key(&mut self, key: Key) -> Action {
        if self.query.typing {
            if key == Key::Enter {
                self.query.typing = false;
                let q = self.query.text.trim().to_string();
                return if q.is_empty() { Action::None } else { Action::Search(q) };
            }
            self.query.key(key);
            return Action::None;
        }
        if self.searched.is_some() {
            // A search's results have the keys until Esc puts them away.
            match key {
                Key::Escape => {
                    self.searched = None;
                    self.hits.clear();
                    self.query.clear();
                }
                Key::Down | Key::Char('j') => self.hit = (self.hit + 1).min(self.hits.len().saturating_sub(1)),
                Key::Up | Key::Char('k') => self.hit = self.hit.saturating_sub(1),
                Key::Char('/') => self.query.start(),
                Key::Char('q') => return Action::Close,
                Key::Enter => {
                    if let Some(h) = self.hits.get(self.hit) {
                        if let Some(path) = file_for(&STANDARDS[h.standard], &self.pdfs) {
                            return Action::OpenAt(path.clone(), h.page);
                        }
                    }
                }
                _ => {}
            }
            return Action::None;
        }
        match key {
            Key::Char('q') | Key::Escape => return Action::Close,
            Key::Down | Key::Char('j') => self.sel = (self.sel + 1).min(STANDARDS.len() - 1),
            Key::Up | Key::Char('k') => self.sel = self.sel.saturating_sub(1),
            Key::Enter => match file_for(&STANDARDS[self.sel], &self.pdfs) {
                Some(p) => return Action::Open(p.clone()),
                None => return Action::Url(STANDARDS[self.sel].url),
            },
            Key::Char('o') => return Action::Url(STANDARDS[self.sel].url),
            Key::Char('f') => return Action::Settings,
            Key::Char('/') => self.query.start(),
            _ => {}
        }
        Action::None
    }
}

pub fn layout(p: &StandardsPage, cols: usize) -> Page {
    let (left, width) = frame(cols, 130);
    let mut g = Grid::new();
    g.put(1, left, "Standards", Role::Title);
    let found = STANDARDS.iter().filter(|s| file_for(s, &p.pdfs).is_some()).count();
    let right = match &p.folder {
        Some(f) => format!("{} · {found} of {} found", f.display(), STANDARDS.len()),
        None => "no folder yet -- f sets ccsds.library".to_string(),
    };
    // A long folder keeps its end (where the PDFs are) and leaves the title be.
    let right = crate::page::fit_tail(&right, width.saturating_sub(12));
    g.put(1, (left + width).saturating_sub(right.chars().count()), &right, if p.folder.is_some() { Role::Muted } else { Role::Warn });
    let (line, role) = p.query.line("search every standard's text", width);
    g.put(2, left, &line, role);
    g.rule(3, left..left + width);
    let mut y = 4;
    if let Some(q) = &p.searched {
        let standards = {
            let mut s: Vec<usize> = p.hits.iter().map(|h| h.standard).collect();
            s.dedup();
            s.len()
        };
        let reading = if p.reading > 0 { format!(" · reading {} more…", p.reading) } else { String::new() };
        let more = if p.hits.len() >= MAX_HITS { " (the first)" } else { "" };
        g.put(y, left, &fit(&format!("\"{q}\" · {} matches{more} in {standards} standards{reading}", p.hits.len()), width), Role::Title);
        y += 2;
        if p.hits.is_empty() && p.reading == 0 {
            g.put(y, left, "Nowhere in the standards found.", Role::Muted);
        }
        for (i, h) in p.hits.iter().enumerate() {
            g.put(y, left, STANDARDS[h.standard].id, Role::Accent);
            g.put(y, left + 19, &format!("p. {}", h.page + 1), Role::Muted);
            g.put(y, left + 29, &fit(&h.context.replace(['\n', '\r'], " "), width.saturating_sub(29)), Role::Text);
            if i == p.hit {
                g.focus(y, left..left + width);
            }
            y += 1;
        }
        g.keys(left, width, &[("Enter", "open at the page"), ("j k", "matches"), ("/", "search again"), ("Esc", "the standards"), ("q", "close")]);
        return g.finish();
    }
    for (i, s) in STANDARDS.iter().enumerate() {
        let file = file_for(s, &p.pdfs);
        g.put(y, left, if file.is_some() { "●" } else { "○" }, if file.is_some() { Role::Good } else { Role::Muted });
        g.put(y, left + 2, s.id, Role::Title);
        g.put(y, left + 21, &fit(s.title, 60), Role::Text);
        let where_ = match file {
            Some(f) => f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            None => "not found -- o opens where to get it".to_string(),
        };
        g.put(y, left + 83, &fit(&where_, width.saturating_sub(83)), Role::Muted);
        if i == p.sel {
            g.focus(y, left..left + width);
        }
        y += 1;
    }
    y += 1;
    g.put(y, left, "A field in the decode (SPC k d) names the standard that defines it: Enter on it opens this PDF at that heading.", Role::Muted);
    g.keys(left, width, &[("Enter", "open"), ("/", "search them all"), ("o", "where to get it"), ("f", "the folder setting"), ("q", "close")]);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_recognized_by_ccsds_and_ecss_naming() {
        let pdfs: Vec<PathBuf> = ["133x0b1c2.pdf", "133x0b2e2.pdf", "ECSS-E-ST-70-41C(15April2016).pdf", "notes.pdf"].iter().map(PathBuf::from).collect();
        assert_eq!(file_for(&STANDARDS[0], &pdfs), Some(&PathBuf::from("133x0b2e2.pdf")), "the newest issue");
        let pus = STANDARDS.iter().find(|s| s.id == "ECSS-E-ST-70-41C").unwrap();
        assert!(file_for(pus, &pdfs).is_some());
        assert!(file_for(&STANDARDS[1], &pdfs).is_none());
        let mut p = StandardsPage::new(None);
        p.folder = Some(PathBuf::from("C:/docs"));
        p.pdfs = pdfs.clone();
        let mut long = StandardsPage::new(None);
        long.folder = Some(PathBuf::from(format!("C:/{}/standards", "deep/".repeat(40))));
        long.pdfs = pdfs;
        let head = layout(&long, 160).text.lines().nth(1).unwrap_or_default().to_string();
        assert!(head.contains("Standards") && head.contains("standards · 2 of 13 found"), "{head}");
        let text = layout(&p, 160).text;
        assert!(text.contains("2 of 13 found") && text.contains("133x0b2e2.pdf"), "{text}");
        assert_eq!(p.key(Key::Enter), Action::Open(PathBuf::from("133x0b2e2.pdf")));
        p.key(Key::Char('j'));
        assert!(matches!(p.key(Key::Enter), Action::Url(_)));
    }

    #[test]
    fn a_search_reads_every_standard_and_opens_a_match_at_its_page() {
        let mut p = StandardsPage::new(None);
        p.pdfs = ["133x0b2e2.pdf", "232x0b4e1.pdf"].iter().map(PathBuf::from).collect();
        assert_eq!(p.searchable().iter().map(|(i, _)| STANDARDS[*i].id).collect::<Vec<_>>(), ["CCSDS 133.0-B", "CCSDS 232.0-B"]);
        p.key(Key::Char('/'));
        assert!(p.typing());
        for c in "segment header".chars() {
            p.key(if c == ' ' { Key::Space } else { Key::Char(c) });
        }
        assert_eq!(p.key(Key::Enter), Action::Search("segment header".into()));
        p.search_started("segment header", 2);
        p.found(2, [(40, "the Segment Header shall".to_string())], false);
        let text = layout(&p, 160).text;
        assert!(text.contains("1 matches in 1 standards · reading 2 more") && text.contains("p. 41"), "{text}");
        p.found(2, [(12, "a segment header".to_string())], true);
        p.found(0, [], true);
        let text = layout(&p, 160).text;
        assert!(text.contains("2 matches in 1 standards") && !text.contains("reading"), "{text}");
        assert_eq!(p.key(Key::Enter), Action::OpenAt(PathBuf::from("232x0b4e1.pdf"), 12), "in page order");
        p.key(Key::Escape);
        assert!(layout(&p, 160).text.contains("CCSDS 301.0-B"), "back to the standards");
    }
}
