//! `SPC k ?`: the standards library. The CCSDS and ECSS standards the
//! mission tools refer to, each with the PDF of it found in the folder
//! `ccsds.library` names (by the CCSDS file naming, `133x0b2e2.pdf` for
//! 133.0-B-2, or the ECSS number in the name). Fenix doesn't ship them:
//! CCSDS publishes its blue books freely, ECSS to registered users.

use std::path::{Path, PathBuf};

use crate::page::{fit, frame, Grid, Key, Page, Role};

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
}

pub struct StandardsPage {
    pub folder: Option<PathBuf>,
    pub pdfs: Vec<PathBuf>,
    sel: usize,
}

impl StandardsPage {
    pub fn new(folder: Option<PathBuf>) -> Self {
        let pdfs = folder.as_deref().map(pdfs_in).unwrap_or_default();
        StandardsPage { folder, pdfs, sel: 0 }
    }

    pub fn key(&mut self, key: Key) -> Action {
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
    g.put(1, (left + width).saturating_sub(right.chars().count()), &fit(&right, width.saturating_sub(12)), if p.folder.is_some() { Role::Muted } else { Role::Warn });
    g.rule(2, left..left + width);
    let mut y = 3;
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
    g.keys(left, width, &[("Enter", "open"), ("o", "where to get it"), ("f", "the folder setting"), ("q", "close")]);
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
        let mut p = StandardsPage { folder: Some(PathBuf::from("C:/docs")), pdfs, sel: 0 };
        let text = layout(&p, 160).text;
        assert!(text.contains("2 of 13 found") && text.contains("133x0b2e2.pdf"), "{text}");
        assert_eq!(p.key(Key::Enter), Action::Open(PathBuf::from("133x0b2e2.pdf")));
        p.key(Key::Char('j'));
        assert!(matches!(p.key(Key::Enter), Action::Url(_)));
    }
}
