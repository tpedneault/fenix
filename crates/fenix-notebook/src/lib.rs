//! Your notebook: notes, journal days and diagrams that Fenix keeps for
//! you, so making one never means choosing a folder.
//!
//! Everything is a plain file under one root -- `notes/*.md`,
//! `journal/YYYY/YYYY-MM-DD.md`, `diagrams/*.mmd` -- beside
//! `index.toml`, which holds what a file can't: its display name, a
//! stable id, pins and where it was exported. Tags and the project a note
//! is about live in the note's own front matter, so they travel with it.
//! A folder made by something else (an Obsidian vault) works as it is:
//! every `.md` and `.mmd` under the root is an entry, and the index is
//! rebuilt from the files whenever it's missing.
//!
//! Nothing here knows about buffers or the screen; `fenix-gui` owns
//! those and calls in to create, save (with history), rename, resolve
//! links and search.

pub mod blocks;
pub mod export;
pub mod meta;
pub mod query;
pub mod templates;

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

pub use meta::{FrontMatter, Heading, Link, Task};

/// What an entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Note,
    Diagram,
    /// A journal day.
    Day,
}

impl Kind {
    pub fn tag(self) -> &'static str {
        match self {
            Kind::Note => "NOTE",
            Kind::Diagram => "DIAG",
            Kind::Day => "DAY",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Kind::Diagram => "mmd",
            _ => "md",
        }
    }
}

pub type EntryId = String;

/// How long one editing session is: a version is kept at its start.
pub const SESSION: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Where an entry was exported, so it can be exported there again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportRecord {
    pub path: PathBuf,
    pub format: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub theme: String,
}

/// One row of `index.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct IndexRow {
    id: EntryId,
    kind: Kind,
    name: String,
    /// Relative to the root, with `/`.
    file: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pinned: bool,
    #[serde(default)]
    created: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    exports: Vec<ExportRecord>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct IndexFile {
    #[serde(default)]
    entry: Vec<IndexRow>,
    /// Saved searches, pinned on the notebook page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    searches: Vec<String>,
}

/// One note, day or diagram.
#[derive(Debug, Clone)]
pub struct Entry {
    pub id: EntryId,
    pub kind: Kind,
    pub name: String,
    /// Absolute.
    pub path: PathBuf,
    pub pinned: bool,
    pub created: String,
    pub modified: Option<SystemTime>,
    pub exports: Vec<ExportRecord>,
    /// The file's text as last read or saved.
    pub text: String,
    pub tags: Vec<String>,
    pub about: Option<String>,
    pub links: Vec<Link>,
}

impl Entry {
    /// For a day: its date.
    pub fn date(&self) -> Option<chrono::NaiveDate> {
        (self.kind == Kind::Day).then(|| day_of(&self.path)).flatten()
    }

    /// The name as a list shows it: a day as `Tue 29 Sep 2026`.
    pub fn display_name(&self) -> String {
        match self.date() {
            Some(d) => d.format("%a %-d %b %Y").to_string(),
            None => self.name.clone(),
        }
    }

    /// Open checkboxes.
    pub fn open_tasks(&self) -> Vec<Task> {
        if self.kind == Kind::Diagram {
            return Vec::new();
        }
        meta::tasks(&self.text).into_iter().filter(|t| !t.done).collect()
    }

    fn refresh(&mut self) {
        if self.kind == Kind::Diagram {
            self.tags = Vec::new();
            self.about = None;
            self.links = Vec::new();
            return;
        }
        let fm = FrontMatter::parse(&self.text);
        self.about = fm.about;
        self.tags = meta::tags(&self.text);
        self.links = meta::links(&self.text);
    }
}

/// What a link resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Entry { id: EntryId, heading: Option<String> },
    /// A file on disk: a path relative to the note, or absolute; a line
    /// (`#L111`) or PDF page (`#page=38`) may follow.
    File { path: PathBuf, fragment: Option<String> },
    /// `project:path/in/it:line`, resolved by the caller.
    ProjectFile { project: String, path: String, line: Option<usize> },
    Url(String),
    /// `mib:NAME`.
    Mib(String),
    /// Something that looks like an issue key: `FSW-212`.
    Task(String),
    /// A note that doesn't exist yet, by the name it would have.
    Missing(String),
}

/// A link to an entry from another one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlink {
    pub from: EntryId,
    pub line: usize,
    /// The line it's on, trimmed.
    pub context: String,
}

#[derive(Debug)]
pub struct Notebook {
    root: PathBuf,
    entries: Vec<Entry>,
    searches: Vec<String>,
    /// When each entry's history was last added to: a new version is
    /// kept once per editing session (`SESSION` of editing, or after
    /// `end_session`).
    snapshotted: HashMap<EntryId, std::time::Instant>,
    pub history_limit: usize,
    counter: u32,
}

/// The file-name form of a name: lower case, words joined by `-`.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.chars() {
        if c.is_alphanumeric() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.extend(c.to_lowercase());
        } else {
            dash = true;
        }
    }
    let out: String = out.chars().take(80).collect();
    if out.is_empty() {
        "untitled".to_string()
    } else {
        out
    }
}

/// The date a journal file is for, from its name.
fn day_of(path: &Path) -> Option<chrono::NaiveDate> {
    let stem = path.file_stem()?.to_str()?;
    chrono::NaiveDate::parse_from_str(stem, "%Y-%m-%d").ok()
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

fn now_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

/// Folders under the root that aren't entries.
fn skipped_dir(name: &str) -> bool {
    name.starts_with('.') || name == "attachments" || name == "node_modules" || name == "target"
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 8 {
        return;
    }
    let Ok(read) = fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        match entry.file_type() {
            Ok(t) if t.is_dir() => {
                if !skipped_dir(&name) {
                    walk(&path, out, depth + 1);
                }
            }
            Ok(t) if t.is_file() => {
                let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
                if matches!(ext.as_deref(), Some("md" | "mmd" | "mermaid")) {
                    out.push(path);
                }
            }
            _ => {}
        }
    }
}

impl Notebook {
    /// Opens (making it if need be) the notebook at `root`.
    pub fn open(root: &Path) -> io::Result<Notebook> {
        fs::create_dir_all(root)?;
        let mut nb = Notebook { root: root.to_path_buf(), entries: Vec::new(), searches: Vec::new(), snapshotted: HashMap::new(), history_limit: 50, counter: 0 };
        nb.rescan()?;
        Ok(nb)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn index_path(&self) -> PathBuf {
        self.root.join("index.toml")
    }

    fn history_dir(&self, id: &str) -> PathBuf {
        self.root.join(".history").join(id)
    }

    /// Where an entry's pasted images go.
    pub fn attachments_dir(&self, id: &str) -> Option<PathBuf> {
        let e = self.get(id)?;
        let stem = e.path.file_stem()?.to_string_lossy().to_string();
        Some(self.root.join("attachments").join(stem))
    }

    /// Reads the index and the files again: files added or removed by
    /// hand are picked up, and the index is rewritten if it changed.
    pub fn rescan(&mut self) -> io::Result<()> {
        let index: IndexFile = fs::read_to_string(self.index_path()).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default();
        self.searches = index.searches;
        let rows: HashMap<String, IndexRow> = index.entry.into_iter().map(|r| (r.file.clone(), r)).collect();
        let mut files = Vec::new();
        walk(&self.root, &mut files, 0);
        files.sort();
        let mut entries = Vec::new();
        let mut changed = false;
        for path in files {
            let file = rel(&self.root, &path);
            let text = fs::read_to_string(&path).unwrap_or_default();
            let modified = fs::metadata(&path).and_then(|m| m.modified()).ok();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            let row = match rows.get(&file) {
                Some(row) => row.clone(),
                None => {
                    changed = true;
                    let kind = if ext == "md" {
                        if day_of(&path).is_some() && file.starts_with("journal/") {
                            Kind::Day
                        } else {
                            Kind::Note
                        }
                    } else {
                        Kind::Diagram
                    };
                    let fm_title = if kind == Kind::Day { None } else { FrontMatter::parse(&text).title.or_else(|| meta::first_title(&text)) };
                    let name = fm_title.unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
                    IndexRow { id: self.new_id(kind), kind, name, file: file.clone(), pinned: false, created: now_stamp(), exports: Vec::new() }
                }
            };
            let mut entry = Entry {
                id: row.id,
                kind: row.kind,
                name: row.name,
                path,
                pinned: row.pinned,
                created: row.created,
                modified,
                exports: row.exports,
                text,
                tags: Vec::new(),
                about: None,
                links: Vec::new(),
            };
            entry.refresh();
            entries.push(entry);
        }
        if rows.len() != entries.len() {
            changed = true;
        }
        self.entries = entries;
        if changed {
            self.write_index()?;
        }
        Ok(())
    }

    fn new_id(&mut self, kind: Kind) -> EntryId {
        self.counter += 1;
        let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let prefix = match kind {
            Kind::Note => "n",
            Kind::Diagram => "d",
            Kind::Day => "j",
        };
        format!("{prefix}_{}{}", radix36(nanos / 1000), radix36(self.counter as u128))
    }

    fn write_index(&self) -> io::Result<()> {
        let file = IndexFile {
            entry: self
                .entries
                .iter()
                .map(|e| IndexRow {
                    id: e.id.clone(),
                    kind: e.kind,
                    name: e.name.clone(),
                    file: rel(&self.root, &e.path),
                    pinned: e.pinned,
                    created: e.created.clone(),
                    exports: e.exports.clone(),
                })
                .collect(),
            searches: self.searches.clone(),
        };
        let text = toml::to_string(&file).map_err(io::Error::other)?;
        fenix_storage::write(&self.index_path(), text.as_bytes())
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    fn get_mut(&mut self, id: &str) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    /// The entry a file is, if it's in the notebook.
    pub fn by_path(&self, path: &Path) -> Option<&Entry> {
        let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let want = canon(path);
        self.entries.iter().find(|e| e.path == path || canon(&e.path) == want)
    }

    /// Whether `path` is inside the notebook's folder.
    pub fn contains(&self, path: &Path) -> bool {
        let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        canon(path).starts_with(canon(&self.root))
    }

    /// By name, ignoring case; then by file name.
    pub fn by_name(&self, name: &str) -> Option<&Entry> {
        let name = name.trim();
        self.entries
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
            .or_else(|| {
                let s = slug(name);
                self.entries.iter().find(|e| e.path.file_stem().map(|f| f.to_string_lossy().eq_ignore_ascii_case(&s)).unwrap_or(false))
            })
            .or_else(|| {
                let date = chrono::NaiveDate::parse_from_str(name, "%Y-%m-%d").ok()?;
                self.entries.iter().find(|e| e.date() == Some(date))
            })
    }

    fn unique_path(&self, dir: &Path, name: &str, ext: &str) -> PathBuf {
        let base = slug(name);
        let mut n = 1;
        loop {
            let file = if n == 1 { format!("{base}.{ext}") } else { format!("{base}-{n}.{ext}") };
            let path = dir.join(&file);
            if !path.exists() && !self.entries.iter().any(|e| e.path == path) {
                return path;
            }
            n += 1;
        }
    }

    /// A new note or diagram called `name` holding `text`.
    pub fn create(&mut self, kind: Kind, name: &str, text: &str) -> io::Result<EntryId> {
        let name = name.trim();
        if name.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "a name is needed"));
        }
        let dir = self.root.join(match kind {
            Kind::Note => "notes",
            Kind::Diagram => "diagrams",
            Kind::Day => "journal",
        });
        fs::create_dir_all(&dir)?;
        let path = self.unique_path(&dir, name, kind.extension());
        fenix_storage::atomic_write_new(&path, |w| w.write_all(text.as_bytes()))?;
        let id = self.new_id(kind);
        let mut entry = Entry {
            id: id.clone(),
            kind,
            name: name.to_string(),
            modified: fs::metadata(&path).and_then(|m| m.modified()).ok(),
            path,
            pinned: false,
            created: now_stamp(),
            exports: Vec::new(),
            text: text.to_string(),
            tags: Vec::new(),
            about: None,
            links: Vec::new(),
        };
        entry.refresh();
        self.entries.push(entry);
        self.snapshotted.insert(id.clone(), std::time::Instant::now());
        self.write_index()?;
        Ok(id)
    }

    /// The journal file for `date`.
    pub fn day_path(&self, date: chrono::NaiveDate) -> PathBuf {
        self.root.join("journal").join(date.format("%Y").to_string()).join(format!("{}.md", date.format("%Y-%m-%d")))
    }

    /// The day's entry, if it's been written.
    pub fn day(&self, date: chrono::NaiveDate) -> Option<&Entry> {
        self.entries.iter().find(|e| e.date() == Some(date))
    }

    /// The day's entry, made from `template` when there isn't one yet.
    /// Whether it was just made, too.
    pub fn ensure_day(&mut self, date: chrono::NaiveDate, template: impl FnOnce() -> String) -> io::Result<(EntryId, bool)> {
        if let Some(e) = self.day(date) {
            return Ok((e.id.clone(), false));
        }
        let path = self.day_path(date);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let text = template();
        if !path.exists() {
            fenix_storage::atomic_write_new(&path, |w| w.write_all(text.as_bytes()))?;
        }
        let id = self.new_id(Kind::Day);
        let text = fs::read_to_string(&path).unwrap_or(text);
        let mut entry = Entry {
            id: id.clone(),
            kind: Kind::Day,
            name: date.format("%Y-%m-%d").to_string(),
            modified: fs::metadata(&path).and_then(|m| m.modified()).ok(),
            path,
            pinned: false,
            created: now_stamp(),
            exports: Vec::new(),
            text,
            tags: Vec::new(),
            about: None,
            links: Vec::new(),
        };
        entry.refresh();
        self.entries.push(entry);
        self.snapshotted.insert(id.clone(), std::time::Instant::now());
        self.write_index()?;
        Ok((id, true))
    }

    /// Keeps the version on disk in the entry's history, once per editing
    /// session: call it before the first write of a session.
    pub fn snapshot(&mut self, id: &str) -> io::Result<()> {
        if self.snapshotted.get(id).is_some_and(|at| at.elapsed() < SESSION) {
            return Ok(());
        }
        let Some(e) = self.get(id) else { return Ok(()) };
        let Ok(old) = fs::read(&e.path) else {
            self.snapshotted.insert(id.to_string(), std::time::Instant::now());
            return Ok(());
        };
        let ext = e.kind.extension();
        let dir = self.history_dir(id);
        fs::create_dir_all(&dir)?;
        // The same text as the newest version isn't a new version.
        let versions = self.history(id);
        let same = versions.first().and_then(|(_, p)| fs::read(p).ok()).map(|t| t == old).unwrap_or(false);
        if !same && !old.is_empty() {
            let stamp = chrono::Local::now().format("%Y-%m-%dT%H-%M-%S").to_string();
            let mut path = dir.join(format!("{stamp}.{ext}"));
            let mut n = 2;
            while path.exists() {
                path = dir.join(format!("{stamp}-{n}.{ext}"));
                n += 1;
            }
            fenix_storage::write(&path, &old)?;
        }
        // Only the newest `history_limit` are kept.
        let versions = self.history(id);
        for (_, old) in versions.iter().skip(self.history_limit.max(1)) {
            let _ = fs::remove_file(old);
        }
        self.snapshotted.insert(id.to_string(), std::time::Instant::now());
        Ok(())
    }

    /// Starts a new editing session for `id`: its next save keeps the
    /// version before it.
    pub fn end_session(&mut self, id: &str) {
        self.snapshotted.remove(id);
    }

    /// An entry's kept versions, newest first: when, and the file.
    pub fn history(&self, id: &str) -> Vec<(String, PathBuf)> {
        let Ok(read) = fs::read_dir(self.history_dir(id)) else { return Vec::new() };
        let mut out: Vec<(String, PathBuf)> = read
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .map(|p| {
                let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let when = chrono::NaiveDateTime::parse_from_str(stem.get(..19).unwrap_or(&stem), "%Y-%m-%dT%H-%M-%S")
                    .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or(stem);
                (when, p)
            })
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1));
        out
    }

    /// Records text Fenix just wrote to (or read from) an entry's file.
    pub fn refresh_text(&mut self, id: &str, text: &str) {
        if let Some(e) = self.get_mut(id) {
            e.text = text.to_string();
            e.modified = fs::metadata(&e.path).and_then(|m| m.modified()).ok();
            e.refresh();
        }
    }

    /// Writes `text` to an entry that isn't open anywhere, keeping its
    /// history.
    pub fn write_text(&mut self, id: &str, text: &str) -> io::Result<()> {
        self.snapshot(id)?;
        let Some(path) = self.get(id).map(|e| e.path.clone()) else { return Ok(()) };
        fenix_storage::write(&path, text.as_bytes())?;
        self.refresh_text(id, text);
        Ok(())
    }

    pub fn set_pinned(&mut self, id: &str, pinned: bool) -> io::Result<()> {
        if let Some(e) = self.get_mut(id) {
            e.pinned = pinned;
        }
        self.write_index()
    }

    pub fn add_export(&mut self, id: &str, record: ExportRecord) -> io::Result<()> {
        if let Some(e) = self.get_mut(id) {
            e.exports.retain(|r| r.path != record.path);
            e.exports.insert(0, record);
        }
        self.write_index()
    }

    pub fn searches(&self) -> &[String] {
        &self.searches
    }

    pub fn toggle_search(&mut self, query: &str) -> io::Result<bool> {
        let pinned = if let Some(i) = self.searches.iter().position(|s| s == query) {
            self.searches.remove(i);
            false
        } else {
            self.searches.push(query.to_string());
            true
        };
        self.write_index()?;
        Ok(pinned)
    }

    /// Renames an entry: its name, its file and every link to it. Returns
    /// the other entries whose text changed, with their new text -- the
    /// caller writes those (into open buffers, or with `write_text`).
    pub fn rename(&mut self, id: &str, new_name: &str) -> io::Result<Vec<(EntryId, String)>> {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "a name is needed"));
        }
        if let Some(other) = self.by_name(new_name) {
            if other.id != id {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} is taken", other.name)));
            }
        }
        let Some(entry) = self.get(id).cloned() else { return Ok(Vec::new()) };
        let old_name = entry.name.clone();
        if entry.kind != Kind::Day {
            let dir = entry.path.parent().map(Path::to_path_buf).unwrap_or_else(|| self.root.clone());
            let new_path = if slug(new_name) == entry.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default() {
                entry.path.clone()
            } else {
                self.unique_path(&dir, new_name, entry.kind.extension())
            };
            if new_path != entry.path {
                fs::rename(&entry.path, &new_path)?;
                let old_att = self.root.join("attachments").join(entry.path.file_stem().unwrap_or_default());
                if old_att.is_dir() {
                    let _ = fs::rename(&old_att, self.root.join("attachments").join(new_path.file_stem().unwrap_or_default()));
                }
            }
            if let Some(e) = self.get_mut(id) {
                e.path = new_path;
                e.name = new_name.to_string();
            }
        }
        let changes = self.rewrite_links(&old_name, new_name);
        self.write_index()?;
        Ok(changes)
    }

    /// The texts that change when links to `old` become links to `new`.
    fn rewrite_links(&self, old: &str, new: &str) -> Vec<(EntryId, String)> {
        let mut out = Vec::new();
        for e in &self.entries {
            if e.kind == Kind::Diagram {
                continue;
            }
            let hits: Vec<&Link> = e.links.iter().filter(|l| !l.markdown && l.target.eq_ignore_ascii_case(old)).collect();
            if hits.is_empty() {
                continue;
            }
            let mut lines: Vec<String> = e.text.lines().map(str::to_string).collect();
            // Right to left, so earlier columns stay put.
            let mut hits = hits;
            hits.sort_by(|a, b| (b.line, b.cols.start).cmp(&(a.line, a.cols.start)));
            for l in hits {
                let Some(line) = lines.get_mut(l.line) else { continue };
                let chars: Vec<char> = line.chars().collect();
                let open = if l.embed { 3 } else { 2 };
                let from = l.cols.start + open;
                let to = from + chars[from..].iter().take_while(|&&c| c != '#' && c != '|' && c != ']').count();
                let lead = chars[from..to].iter().take_while(|c| c.is_whitespace()).count();
                let mut s: String = chars[..from + lead].iter().collect();
                s.push_str(new);
                s.extend(chars[to..].iter());
                *line = s;
            }
            let mut text = lines.join("\n");
            if e.text.ends_with('\n') {
                text.push('\n');
            }
            if text != e.text {
                out.push((e.id.clone(), text));
            }
        }
        out
    }

    /// Moves an entry, its attachments and its history to the Recycle Bin.
    pub fn delete(&mut self, id: &str) -> io::Result<()> {
        let Some(entry) = self.get(id).cloned() else { return Ok(()) };
        let mut paths = vec![entry.path.clone()];
        let att = self.root.join("attachments").join(entry.path.file_stem().unwrap_or_default());
        if att.is_dir() {
            paths.push(att);
        }
        let hist = self.history_dir(id);
        if hist.is_dir() {
            paths.push(hist);
        }
        let outcomes = fenix_fs::to_recycle_bin(&paths);
        if let Some(bad) = outcomes.iter().find(|o| !o.succeeded()) {
            if entry.path.exists() {
                return Err(io::Error::other(format!("couldn't move it to the Recycle Bin: {}", bad.error.clone().unwrap_or_default())));
            }
        }
        self.entries.retain(|e| e.id != id);
        self.write_index()
    }

    /// Puts a kept version back (keeping the current one in the history).
    pub fn restore(&mut self, id: &str, version: &Path) -> io::Result<String> {
        let text = fs::read_to_string(version)?;
        self.end_session(id);
        self.write_text(id, &text)?;
        Ok(text)
    }

    /// Moves a note or diagram out of the notebook into `dir` (a
    /// project's docs folder), with its attachments. The new path.
    pub fn move_out(&mut self, id: &str, dir: &Path) -> io::Result<PathBuf> {
        let Some(entry) = self.get(id).cloned() else { return Err(io::Error::new(io::ErrorKind::NotFound, "no such entry")) };
        fs::create_dir_all(dir)?;
        let file = entry.path.file_name().unwrap_or_default();
        let mut dest = dir.join(file);
        let mut n = 2;
        while dest.exists() {
            dest = dir.join(format!("{}-{n}.{}", entry.path.file_stem().unwrap_or_default().to_string_lossy(), entry.kind.extension()));
            n += 1;
        }
        let mut text = entry.text.clone();
        let att = self.root.join("attachments").join(entry.path.file_stem().unwrap_or_default());
        if att.is_dir() {
            let dest_att = dir.join(format!("{}_files", dest.file_stem().unwrap_or_default().to_string_lossy()));
            copy_dir(&att, &dest_att)?;
            let from = format!("attachments/{}/", entry.path.file_stem().unwrap_or_default().to_string_lossy());
            let to = format!("{}_files/", dest.file_stem().unwrap_or_default().to_string_lossy());
            text = text.replace(&format!("../{from}"), &to).replace(&from, &to);
        }
        fenix_storage::atomic_write_new(&dest, |w| w.write_all(text.as_bytes()))?;
        self.delete(id)?;
        Ok(dest)
    }

    /// Copies a file into the notebook as a new entry.
    pub fn import(&mut self, path: &Path) -> io::Result<EntryId> {
        let text = fs::read_to_string(path)?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let kind = if ext == "mmd" || ext == "mermaid" { Kind::Diagram } else { Kind::Note };
        let name = if kind == Kind::Note { FrontMatter::parse(&text).title.or_else(|| meta::first_title(&text)) } else { None };
        let name = name.unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "imported".into()));
        let name = self.free_name(&name);
        self.create(kind, &name, &text)
    }

    /// `name`, or `name 2`, `name 3`... whichever isn't taken.
    pub fn free_name(&self, name: &str) -> String {
        if self.by_name(name).is_none() {
            return name.to_string();
        }
        (2..).map(|n| format!("{name} {n}")).find(|n| self.by_name(n).is_none()).unwrap_or_else(|| name.to_string())
    }

    /// What `link`, written in entry `from`, points at.
    pub fn resolve(&self, link: &Link, from: Option<&Path>) -> Target {
        let t = link.target.trim();
        if t.is_empty() {
            return match (link.heading.clone(), from.and_then(|p| self.by_path(p))) {
                (heading, Some(e)) => Target::Entry { id: e.id.clone(), heading },
                (Some(h), None) => Target::Missing(h),
                (None, None) => Target::Missing(String::new()),
            };
        }
        if t.contains("://") || t.starts_with("mailto:") {
            return Target::Url(t.to_string());
        }
        if let Some(name) = t.strip_prefix("mib:") {
            return Target::Mib(name.trim().to_string());
        }
        if !link.markdown {
            if let Some(e) = self.by_name(t) {
                return Target::Entry { id: e.id.clone(), heading: link.heading.clone() };
            }
            // project:path/to/file:line
            if let Some((project, rest)) = t.split_once(':') {
                if !project.is_empty() && !project.contains(['/', '\\', ' ']) && project.len() > 1 && !rest.is_empty() {
                    let (path, line) = match rest.rsplit_once(':') {
                        Some((p, l)) if l.chars().all(|c| c.is_ascii_digit()) && !l.is_empty() => (p.to_string(), l.parse().ok()),
                        _ => (rest.to_string(), None),
                    };
                    return Target::ProjectFile { project: project.to_string(), path, line };
                }
            }
            if is_issue_key(t) {
                return Target::Task(t.to_string());
            }
            let lower = t.to_ascii_lowercase();
            if [".pdf", ".png", ".jpg", ".jpeg", ".svg", ".txt", ".c", ".h", ".rs", ".py", ".tcl"].iter().any(|ext| lower.ends_with(ext)) {
                return self.file_target(t, link.heading.clone(), from);
            }
            return Target::Missing(t.to_string());
        }
        // A Markdown link: a file, maybe a note in the notebook.
        let file = self.file_target(t, link.heading.clone(), from);
        if let Target::File { path, fragment } = &file {
            if let Some(e) = self.by_path(path) {
                return Target::Entry { id: e.id.clone(), heading: fragment.clone() };
            }
        }
        file
    }

    fn file_target(&self, t: &str, fragment: Option<String>, from: Option<&Path>) -> Target {
        let decoded = t.replace("%20", " ");
        let p = PathBuf::from(&decoded);
        let path = if p.is_absolute() { p } else { from.and_then(Path::parent).unwrap_or(&self.root).join(p) };
        Target::File { path, fragment }
    }

    /// Links to `id` from every other entry.
    pub fn backlinks(&self, id: &str) -> Vec<Backlink> {
        let mut out = Vec::new();
        for e in &self.entries {
            if e.id == id {
                continue;
            }
            for l in &e.links {
                if let Target::Entry { id: to, .. } = self.resolve(l, Some(&e.path)) {
                    if to == id {
                        let context = e.text.lines().nth(l.line).unwrap_or("").trim().to_string();
                        out.push(Backlink { from: e.id.clone(), line: l.line, context });
                    }
                }
            }
        }
        out
    }

    /// Places another entry names `id` in plain text, without a link.
    pub fn unlinked_mentions(&self, id: &str) -> Vec<Backlink> {
        let Some(target) = self.get(id) else { return Vec::new() };
        if target.name.chars().count() < 3 {
            return Vec::new();
        }
        let needle = target.name.to_lowercase();
        let mut out = Vec::new();
        for e in &self.entries {
            if e.id == id || e.kind == Kind::Diagram {
                continue;
            }
            for (n, line) in meta::prose_lines(&e.text) {
                let lower = line.to_lowercase();
                let Some(at) = lower.find(&needle) else { continue };
                // Inside a link already?
                let before = &lower[..at];
                if before.rfind("[[").map(|o| before[o..].find("]]").is_none()).unwrap_or(false) {
                    continue;
                }
                out.push(Backlink { from: e.id.clone(), line: n, context: line.trim().to_string() });
            }
        }
        out
    }

    /// Every tag, with how many entries have it; most used first.
    pub fn tags(&self) -> Vec<(String, usize)> {
        let mut counts: HashMap<String, (String, usize)> = HashMap::new();
        for e in &self.entries {
            for t in &e.tags {
                let slot = counts.entry(t.to_lowercase()).or_insert_with(|| (t.clone(), 0));
                slot.1 += 1;
            }
        }
        let mut out: Vec<(String, usize)> = counts.into_values().collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        out
    }

    /// Entries about `project` (by its name or folder name).
    pub fn about(&self, project: &str) -> Vec<&Entry> {
        self.entries.iter().filter(|e| e.about.as_deref().map(|a| a.eq_ignore_ascii_case(project)).unwrap_or(false)).collect()
    }

    /// Entries, most recently changed first; pinned ones on top.
    pub fn recent(&self) -> Vec<&Entry> {
        let mut out: Vec<&Entry> = self.entries.iter().collect();
        out.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.modified.cmp(&a.modified)));
        out
    }
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)?.flatten() {
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

/// `ABC-123`: letters, a dash, digits.
pub fn is_issue_key(s: &str) -> bool {
    let Some((a, b)) = s.split_once('-') else { return false };
    !a.is_empty() && a.len() <= 10 && a.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) && a.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && !b.is_empty() && b.chars().all(|c| c.is_ascii_digit())
}

fn radix36(mut n: u128) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}


#[cfg(test)]
mod tests {
    use super::*;

    fn nb() -> (tempfile::TempDir, Notebook) {
        let dir = tempfile::tempdir().unwrap();
        let nb = Notebook::open(dir.path()).unwrap();
        (dir, nb)
    }

    #[test]
    fn slugs_are_lower_case_words_joined_by_dashes() {
        assert_eq!(slug("TC uplink: design notes"), "tc-uplink-design-notes");
        assert_eq!(slug("Ground ↔ OBC"), "ground-obc");
        assert_eq!(slug("  ?? "), "untitled");
    }

    #[test]
    fn creating_writes_a_file_named_after_it_and_indexes_it() {
        let (dir, mut nb) = nb();
        let id = nb.create(Kind::Note, "TC uplink: design notes", "# TC uplink\n#tc\n").unwrap();
        let e = nb.get(&id).unwrap();
        assert_eq!(e.path, dir.path().join("notes").join("tc-uplink-design-notes.md"));
        assert_eq!(e.tags, vec!["tc"]);
        let again = nb.create(Kind::Note, "TC uplink: design notes!", "").unwrap();
        assert!(nb.get(&again).unwrap().path.ends_with("tc-uplink-design-notes-2.md"));
        // A fresh open reads the same names and ids back.
        let back = Notebook::open(dir.path()).unwrap();
        assert_eq!(back.get(&id).unwrap().name, "TC uplink: design notes");
        assert_eq!(back.entries().len(), 2);
    }

    #[test]
    fn files_added_by_hand_are_named_from_their_title() {
        let (dir, _) = nb();
        fs::create_dir_all(dir.path().join("notes")).unwrap();
        fs::write(dir.path().join("notes").join("x.md"), "---\ntitle: Bench log\n---\n").unwrap();
        fs::write(dir.path().join("loose.md"), "# Loose note\n").unwrap();
        fs::create_dir_all(dir.path().join("journal/2026")).unwrap();
        fs::write(dir.path().join("journal/2026/2026-09-28.md"), "log\n").unwrap();
        fs::create_dir_all(dir.path().join(".obsidian")).unwrap();
        fs::write(dir.path().join(".obsidian").join("skip.md"), "").unwrap();
        let nb = Notebook::open(dir.path()).unwrap();
        let mut names: Vec<(Kind, String)> = nb.entries().iter().map(|e| (e.kind, e.name.clone())).collect();
        names.sort_by(|a, b| a.1.cmp(&b.1));
        assert_eq!(names, vec![(Kind::Day, "2026-09-28".into()), (Kind::Note, "Bench log".into()), (Kind::Note, "Loose note".into())]);
        assert_eq!(nb.by_name("2026-09-28").unwrap().display_name(), "Mon 28 Sep 2026");
    }

    #[test]
    fn history_keeps_the_version_before_each_session() {
        let (_dir, mut nb) = nb();
        let id = nb.create(Kind::Note, "N", "one\n").unwrap();
        // Made this session: nothing older to keep.
        nb.write_text(&id, "two\n").unwrap();
        assert!(nb.history(&id).is_empty());
        nb.end_session(&id);
        nb.write_text(&id, "three\n").unwrap();
        nb.write_text(&id, "four\n").unwrap();
        let h = nb.history(&id);
        assert_eq!(h.len(), 1);
        assert_eq!(fs::read_to_string(&h[0].1).unwrap(), "two\n");
        let text = nb.restore(&id, &h[0].1.clone()).unwrap();
        assert_eq!(text, "two\n");
        assert_eq!(fs::read_to_string(&nb.get(&id).unwrap().path).unwrap(), "two\n");
        assert_eq!(nb.history(&id).len(), 2);
    }

    #[test]
    fn renaming_moves_the_file_and_rewrites_links_to_it() {
        let (_dir, mut nb) = nb();
        let a = nb.create(Kind::Note, "Uplink", "# Uplink\n").unwrap();
        let b = nb.create(Kind::Note, "Log", "see [[uplink#Retries|the retries]] and ![[Uplink]]\n").unwrap();
        let changes = nb.rename(&a, "TC uplink").unwrap();
        assert!(nb.get(&a).unwrap().path.ends_with("tc-uplink.md"));
        assert_eq!(changes, vec![(b.clone(), "see [[TC uplink#Retries|the retries]] and ![[TC uplink]]\n".to_string())]);
        assert!(nb.rename(&b, "tc uplink").is_err());
    }

    #[test]
    fn links_resolve_to_entries_files_projects_and_tasks() {
        let (dir, mut nb) = nb();
        let a = nb.create(Kind::Note, "ADR 007", "").unwrap();
        let from = dir.path().join("notes").join("x.md");
        let r = |nb: &Notebook, text: &str| nb.resolve(&meta::links(text)[0], Some(&from));
        assert_eq!(r(&nb, "[[adr 007#Decision]]"), Target::Entry { id: a.clone(), heading: Some("Decision".into()) });
        assert_eq!(r(&nb, "[[Nope]]"), Target::Missing("Nope".into()));
        assert_eq!(r(&nb, "[[FSW-212]]"), Target::Task("FSW-212".into()));
        assert_eq!(r(&nb, "[[mib:ZAB12345]]"), Target::Mib("ZAB12345".into()));
        assert_eq!(r(&nb, "[[fenix-sat:src/uplink.c:111]]"), Target::ProjectFile { project: "fenix-sat".into(), path: "src/uplink.c".into(), line: Some(111) });
        assert_eq!(r(&nb, "[[spec.pdf#page=38]]"), Target::File { path: dir.path().join("notes").join("spec.pdf"), fragment: Some("page=38".into()) });
        assert_eq!(r(&nb, "[x](adr-007.md)"), Target::Entry { id: a, heading: None });
    }

    #[test]
    fn backlinks_and_unlinked_mentions() {
        let (_dir, mut nb) = nb();
        let a = nb.create(Kind::Note, "Uplink notes", "").unwrap();
        let b = nb.create(Kind::Note, "Log", "intro\nsee [[Uplink notes]] here\n").unwrap();
        let c = nb.create(Kind::Note, "Other", "the uplink notes need work\n").unwrap();
        assert_eq!(nb.backlinks(&a), vec![Backlink { from: b, line: 1, context: "see [[Uplink notes]] here".into() }]);
        let m = nb.unlinked_mentions(&a);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].from, c);
    }

    #[test]
    fn days_are_made_once_from_their_template() {
        let (_dir, mut nb) = nb();
        let date = chrono::NaiveDate::from_ymd_opt(2026, 9, 29).unwrap();
        let (id, made) = nb.ensure_day(date, || "# Tuesday\n".into()).unwrap();
        assert!(made);
        let (again, made) = nb.ensure_day(date, || unreachable!()).unwrap();
        assert!(!made);
        assert_eq!(id, again);
        assert!(nb.get(&id).unwrap().path.ends_with("journal/2026/2026-09-29.md"));
    }

    #[test]
    fn deleting_and_pinning() {
        let (_dir, mut nb) = nb();
        let a = nb.create(Kind::Note, "A", "a").unwrap();
        let b = nb.create(Kind::Note, "B", "b").unwrap();
        nb.set_pinned(&b, true).unwrap();
        assert_eq!(nb.recent()[0].id, b);
        let path = nb.get(&a).unwrap().path.clone();
        nb.delete(&a).unwrap();
        assert!(nb.get(&a).is_none());
        assert!(!path.exists());
    }

    #[test]
    fn issue_keys() {
        assert!(is_issue_key("FSW-212"));
        assert!(!is_issue_key("fsw-212"));
        assert!(!is_issue_key("FSW-"));
        assert!(!is_issue_key("Hello"));
    }
}
