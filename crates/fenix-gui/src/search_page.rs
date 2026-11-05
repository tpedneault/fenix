//! The Search page (`SPC /`, `SPC s p`, `SPC s r`): a Search field with
//! its toggles and scope, a Replace field that opens when it's wanted,
//! and what was found grouped by file -- each match with its line, lit,
//! and, while Replace is open, what it becomes. Matches and whole files
//! can be left out of a replace. This lays the page out and maps keys to
//! actions; searching, opening and replacing are the host's
//! (`app/search_host.rs`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use fenix_project::search::{FileResult, Line, MatchAt, Options, Pattern};

use crate::page::{edit_line, fit, insert_at, with_caret, Grid, Key, Page, Role};

/// A one-line field and its caret.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Input {
    pub text: String,
    pub caret: usize,
}

impl Input {
    pub fn new(text: &str) -> Self {
        Input { text: text.to_string(), caret: text.chars().count() }
    }

    fn set(&mut self, text: &str) {
        *self = Input::new(text);
    }
}

/// Which field has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Query,
    Replace,
    Globs,
}

/// Where the search looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ScopeKind {
    Project,
    /// The folder it was started from (`SPC e g`).
    Folder,
    /// Every open file, unsaved text included.
    OpenFiles,
    /// The file it was started from.
    ThisFile,
    /// The lines selected when it was started.
    Selection,
    /// The files Git says changed.
    Changed,
}

impl ScopeKind {
    pub fn label(self) -> &'static str {
        match self {
            ScopeKind::Project => "project",
            ScopeKind::Folder => "folder",
            ScopeKind::OpenFiles => "open files",
            ScopeKind::ThisFile => "this file",
            ScopeKind::Selection => "selection",
            ScopeKind::Changed => "changed",
        }
    }
}

/// How the search is going.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Status {
    /// Nothing typed yet.
    #[default]
    Idle,
    Searching { files: usize },
    Done { files: usize, ms: u128, binary: usize, truncated: bool },
    Failed(String),
}

/// One file's results, and what's left out of a replace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultFile {
    pub path: PathBuf,
    /// Shown relative to the project.
    pub rel: String,
    pub lines: Vec<Line>,
    pub folded: bool,
    /// The whole file is left out.
    pub left_out: bool,
    /// Matches left out, by (line, which on the line).
    pub skipped: HashSet<MatchAt>,
}

impl ResultFile {
    pub fn new(file: FileResult, root: &Path) -> Self {
        let rel = file.path.strip_prefix(root).unwrap_or(&file.path).to_string_lossy().replace('\\', "/");
        ResultFile { path: file.path, rel, lines: file.lines, folded: false, left_out: false, skipped: HashSet::new() }
    }

    pub fn matches(&self) -> usize {
        self.lines.iter().map(|l| l.matches.len()).sum()
    }

    /// The matches a replace would change.
    pub fn included(&self) -> usize {
        if self.left_out {
            return 0;
        }
        self.lines.iter().map(|l| (0..l.matches.len()).filter(|&n| !self.skipped.contains(&(l.number, n))).count()).sum()
    }

    fn line_left_out(&self, line: &Line) -> bool {
        self.left_out || (!line.matches.is_empty() && (0..line.matches.len()).all(|n| self.skipped.contains(&(line.number, n))))
    }
}

/// A row of results: a file, or one of its lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    File(usize),
    Line(usize, usize),
}

/// What a key asks of the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    /// The query or its options changed: search again now.
    Search,
    /// The query is being typed: search again in a moment.
    SearchSoon,
    /// Go to this match, the keyboard with it.
    Open { path: PathBuf, line: usize, col: usize },
    /// The selection moved: what it's on, to show while following.
    Show { path: PathBuf, line: usize, col: usize },
    /// `p`: following the selection beside it, on or off.
    Follow(bool),
    /// `y` to the replace question: replace what's in.
    Apply,
    /// `Alt-s`: the next scope.
    NextScope,
}

#[derive(Debug, Clone, Default)]
pub struct SearchPage {
    pub query: Input,
    pub replace: Input,
    pub globs: Input,
    pub replace_open: bool,
    pub editing: Option<Field>,
    pub options: Options,
    pub preserve_case: bool,
    pub scope: Option<ScopeKind>,
    /// The scopes this page can use (`folder`, `this file`, `selection`
    /// only when it was started from one).
    pub scopes: Vec<ScopeKind>,
    /// What the narrower scopes are: the folder, the file, the lines.
    pub folder: Option<PathBuf>,
    pub file: Option<PathBuf>,
    pub selection: Option<(usize, usize)>,
    /// The project, which paths are shown under.
    pub root: PathBuf,
    /// The compiled query the results came from: what a replace shows.
    pub pattern: Option<Pattern>,
    pub files: Vec<ResultFile>,
    pub status: Status,
    /// A bad regex: why, under the field.
    pub error: Option<String>,
    pub cursor: usize,
    pub pending_g: bool,
    pub pending_z: bool,
    /// The selection is shown beside the page as it moves.
    pub following: bool,
    /// The match `SPC p n`/`N` are on: (file, line) indices.
    pub current: Option<(usize, usize)>,
    /// The replace question is up.
    pub confirm: bool,
    /// What the host last said (a replace done, nothing to undo).
    pub message: Option<String>,
    /// This project's searches, newest first, and where `↑` has got to.
    pub history: Vec<String>,
    pub history_at: Option<usize>,
}

impl SearchPage {
    #[cfg(test)]
    pub fn new(root: PathBuf, scopes: Vec<ScopeKind>, scope: ScopeKind) -> Self {
        SearchPage { root, scope: Some(scope), scopes, following: true, ..Default::default() }
    }

    /// Whether a field has the keyboard (every key is the page's then).
    pub fn typing(&self) -> bool {
        self.editing.is_some()
    }

    pub fn scope(&self) -> ScopeKind {
        self.scope.unwrap_or(ScopeKind::Project)
    }

    /// Starts typing into `field` (opening Replace if that's the one).
    pub fn edit(&mut self, field: Field) {
        if field == Field::Replace {
            self.replace_open = true;
        }
        self.editing = Some(field);
        self.history_at = None;
    }

    /// The text pasted into the field being typed.
    pub fn paste(&mut self, text: &str) -> Action {
        let Some(field) = self.editing else { return Action::None };
        let input = self.input_mut(field);
        insert_at(&mut input.text, &mut input.caret, text.lines().next().unwrap_or(""));
        self.changed(field)
    }

    fn input_mut(&mut self, field: Field) -> &mut Input {
        match field {
            Field::Query => &mut self.query,
            Field::Replace => &mut self.replace,
            Field::Globs => &mut self.globs,
        }
    }

    /// A field's text changed: the Replace field only redraws.
    fn changed(&mut self, field: Field) -> Action {
        match field {
            Field::Replace => Action::None,
            Field::Query | Field::Globs => Action::SearchSoon,
        }
    }

    /// The options a search runs with: the toggles, and the globs typed.
    pub fn search_options(&self) -> Options {
        let globs = self.globs.text.split([' ', ',']).map(str::trim).filter(|g| !g.is_empty()).map(str::to_string).collect();
        Options { globs, ..self.options.clone() }
    }

    /// `Alt-c`, `Alt-w`, `Alt-r`, `Alt-h`, `Alt-p`: a toggle flipped.
    pub fn toggle(&mut self, which: char) -> Action {
        match which {
            'c' => self.options.match_case = !self.options.match_case,
            'w' => self.options.whole_word = !self.options.whole_word,
            'r' => self.options.regex = !self.options.regex,
            'h' => self.options.hidden = !self.options.hidden,
            'p' => {
                self.preserve_case = !self.preserve_case;
                return Action::None;
            }
            's' => return Action::NextScope,
            _ => return Action::None,
        }
        Action::Search
    }

    /// The scope after this one, of those the page can use.
    pub fn next_scope(&mut self) {
        let at = self.scopes.iter().position(|&s| s == self.scope()).unwrap_or(0);
        self.scope = self.scopes.get((at + 1) % self.scopes.len().max(1)).copied().or(self.scope);
    }

    /// A new search began: the old results go.
    pub fn begin(&mut self, pattern: Option<Pattern>) {
        self.pattern = pattern;
        self.files.clear();
        self.cursor = 0;
        self.current = None;
        self.confirm = false;
        self.status = if self.pattern.is_some() { Status::Searching { files: 0 } } else { Status::Idle };
    }

    /// Files found by the search under way. The first results put the
    /// cursor on the first match, unless the query's still being typed.
    pub fn push(&mut self, found: Vec<FileResult>, searched: usize) {
        let root = self.root.clone();
        let first = self.files.is_empty();
        self.files.extend(found.into_iter().map(|f| ResultFile::new(f, &root)));
        if first && self.editing.is_none() && self.cursor == 0 {
            self.cursor = self.rows().iter().position(|&r| matches!(r, Row::Line(..)) && self.stops(r)).unwrap_or(0);
        }
        if let Status::Searching { files } = &mut self.status {
            *files = searched;
        }
    }

    pub fn total(&self) -> usize {
        self.files.iter().map(ResultFile::matches).sum()
    }

    pub fn included(&self) -> (usize, usize) {
        let files = self.files.iter().filter(|f| f.included() > 0).count();
        (self.files.iter().map(ResultFile::included).sum(), files)
    }

    /// The rows shown, folded files' lines left out.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (fi, file) in self.files.iter().enumerate() {
            rows.push(Row::File(fi));
            if !file.folded {
                rows.extend((0..file.lines.len()).map(|li| Row::Line(fi, li)));
            }
        }
        rows
    }

    /// Whether the cursor can rest on `row`: a file or a match, not context.
    fn stops(&self, row: Row) -> bool {
        match row {
            Row::File(_) => true,
            Row::Line(fi, li) => !self.files[fi].lines[li].matches.is_empty(),
        }
    }

    /// The match the cursor is on (its file's first one, on a file row).
    pub fn selected(&self) -> Option<(PathBuf, usize, usize)> {
        let rows = self.rows();
        let (fi, li) = match rows.get(self.cursor)? {
            Row::File(fi) => (*fi, self.files[*fi].lines.iter().position(|l| !l.matches.is_empty())?),
            Row::Line(fi, li) => (*fi, *li),
        };
        let line = &self.files[fi].lines[li];
        let col = line.matches.first().map(|m| line.text[..m.start].chars().count() + 1).unwrap_or(1);
        Some((self.files[fi].path.clone(), line.number, col))
    }

    fn show_selected(&self) -> Action {
        match self.selected() {
            Some((path, line, col)) => Action::Show { path, line, col },
            None => Action::None,
        }
    }

    fn move_cursor(&mut self, by: isize) -> Action {
        let rows = self.rows();
        let mut at = self.cursor as isize;
        loop {
            at += by.signum();
            if at < 0 || at as usize >= rows.len() {
                return Action::None;
            }
            if self.stops(rows[at as usize]) {
                break;
            }
        }
        self.cursor = at as usize;
        self.show_selected()
    }

    /// Every match, in order, as (file, line) indices -- what `SPC p
    /// n`/`N` step through.
    pub fn match_rows(&self) -> Vec<(usize, usize)> {
        self.files.iter().enumerate().flat_map(|(fi, f)| f.lines.iter().enumerate().filter(|(_, l)| !l.matches.is_empty()).map(move |(li, _)| (fi, li))).collect()
    }

    /// `SPC p n`/`N` moved to match `(fi, li)`: lit, the cursor on it.
    pub fn set_current(&mut self, at: (usize, usize)) {
        self.current = Some(at);
        if let Some(file) = self.files.get_mut(at.0) {
            file.folded = false;
        }
        if let Some(row) = self.rows().iter().position(|&r| r == Row::Line(at.0, at.1)) {
            self.cursor = row;
        }
    }

    pub fn key(&mut self, key: Key) -> Action {
        self.message = None;
        if self.confirm {
            self.confirm = false;
            return if key == Key::Char('y') { Action::Apply } else { Action::None };
        }
        if let Some(field) = self.editing {
            return self.field_key(field, key);
        }
        let rows = self.rows();
        let g = std::mem::take(&mut self.pending_g);
        let z = std::mem::take(&mut self.pending_z);
        if z {
            return match key {
                Key::Char('a') => self.fold_here(&rows, None),
                Key::Char('M') => self.fold_all(true),
                Key::Char('R') => self.fold_all(false),
                _ => Action::None,
            };
        }
        match key {
            Key::Down | Key::Char('j') => self.move_cursor(1),
            Key::Up | Key::Char('k') => self.move_cursor(-1),
            Key::Char('g') if g => {
                self.cursor = 0;
                self.show_selected()
            }
            Key::Char('g') => {
                self.pending_g = true;
                Action::None
            }
            Key::Char('G') => {
                self.cursor = rows.iter().rposition(|&r| self.stops(r)).unwrap_or(0);
                self.show_selected()
            }
            Key::Char('z') => {
                self.pending_z = true;
                Action::None
            }
            Key::Tab => self.fold_here(&rows, None),
            Key::Enter => match self.selected() {
                Some((path, line, col)) => Action::Open { path, line, col },
                None => Action::None,
            },
            Key::Char('p') => {
                self.following = !self.following;
                Action::Follow(self.following)
            }
            Key::Char('/' | 'i') => {
                self.edit(Field::Query);
                Action::None
            }
            Key::Char('r') => {
                self.edit(Field::Replace);
                Action::None
            }
            Key::Char('f') => {
                self.edit(Field::Globs);
                Action::None
            }
            Key::Char('x') => self.leave_out(&rows, false),
            Key::Char('X') => self.leave_out(&rows, true),
            Key::Char('R') => {
                if !self.replace_open {
                    self.edit(Field::Replace);
                    return Action::None;
                }
                let (n, _) = self.included();
                if n == 0 {
                    self.message = Some("nothing to replace".into());
                } else {
                    self.confirm = true;
                }
                Action::None
            }
            Key::Char('+') => {
                self.options.context = (self.options.context + 1).min(9);
                Action::Search
            }
            Key::Char('-') => {
                self.options.context = self.options.context.saturating_sub(1);
                Action::Search
            }
            Key::Escape if self.replace_open => {
                self.replace_open = false;
                Action::None
            }
            Key::Char('q') | Key::Escape => Action::Close,
            _ => Action::None,
        }
    }

    fn field_key(&mut self, field: Field, key: Key) -> Action {
        match key {
            Key::Enter => {
                self.editing = None;
                self.cursor = self.rows().iter().position(|&r| self.stops(r) && matches!(r, Row::Line(..))).unwrap_or(0);
                Action::Search
            }
            Key::Escape => {
                self.editing = None;
                Action::None
            }
            Key::Tab | Key::BackTab => {
                let order: Vec<Field> = if self.replace_open { vec![Field::Query, Field::Replace, Field::Globs] } else { vec![Field::Query, Field::Globs] };
                let at = order.iter().position(|&f| f == field).unwrap_or(0);
                let next = if key == Key::Tab { at + 1 } else { at + order.len() - 1 };
                self.editing = if key == Key::Tab && next >= order.len() { None } else { Some(order[next % order.len()]) };
                Action::None
            }
            Key::Up | Key::Down if field == Field::Query => {
                if self.history.is_empty() {
                    return Action::None;
                }
                let at = match (self.history_at, key) {
                    (None, Key::Up) => Some(0),
                    (None, _) => None,
                    (Some(i), Key::Up) => Some((i + 1).min(self.history.len() - 1)),
                    (Some(0), _) => None,
                    (Some(i), _) => Some(i - 1),
                };
                self.history_at = at;
                let text = at.map(|i| self.history[i].clone()).unwrap_or_default();
                self.query.set(&text);
                Action::SearchSoon
            }
            key => {
                let input = self.input_mut(field);
                let before = input.text.clone();
                if !edit_line(&mut input.text, &mut input.caret, key) || input.text == before {
                    return Action::None;
                }
                self.changed(field)
            }
        }
    }

    /// `Tab`/`za`: the file the cursor is in folds or opens.
    fn fold_here(&mut self, rows: &[Row], to: Option<bool>) -> Action {
        let fi = match rows.get(self.cursor) {
            Some(Row::File(fi) | Row::Line(fi, _)) => *fi,
            None => return Action::None,
        };
        let file = &mut self.files[fi];
        file.folded = to.unwrap_or(!file.folded);
        self.cursor = self.rows().iter().position(|&r| r == Row::File(fi)).unwrap_or(0);
        Action::None
    }

    fn fold_all(&mut self, folded: bool) -> Action {
        let fi = match self.rows().get(self.cursor) {
            Some(Row::File(fi) | Row::Line(fi, _)) => Some(*fi),
            None => None,
        };
        for file in &mut self.files {
            file.folded = folded;
        }
        self.cursor = fi.and_then(|fi| self.rows().iter().position(|&r| r == Row::File(fi))).unwrap_or(0);
        Action::None
    }

    /// `x`: the line's matches in or out (a file row: the file); `X`: the
    /// file.
    fn leave_out(&mut self, rows: &[Row], whole_file: bool) -> Action {
        match rows.get(self.cursor).copied() {
            Some(Row::File(fi)) => self.files[fi].left_out = !self.files[fi].left_out,
            Some(Row::Line(fi, _)) if whole_file => self.files[fi].left_out = !self.files[fi].left_out,
            Some(Row::Line(fi, li)) => {
                let file = &mut self.files[fi];
                let line = &file.lines[li];
                let keys: Vec<MatchAt> = (0..line.matches.len()).map(|n| (line.number, n)).collect();
                if keys.iter().all(|k| file.skipped.contains(k)) {
                    for k in &keys {
                        file.skipped.remove(k);
                    }
                } else {
                    file.skipped.extend(keys);
                }
                // On to the next match, as a review goes.
                return self.move_cursor(1);
            }
            None => {}
        }
        Action::None
    }
}

/// Lays the page out `cols` wide and, when known, `height` tall.
pub fn layout(page: &SearchPage, cols: usize, height: Option<usize>) -> Page {
    let mut g = Grid::new();
    let left = 1;
    let width = cols.saturating_sub(2).max(24);
    let label_w = 9;
    let mut line = 0;

    // The fields.
    let toggles: Vec<(&str, bool)> = vec![("Aa", page.options.match_case), ("ab", page.options.whole_word), (".*", page.options.regex)];
    let toggles_w: usize = toggles.iter().map(|(t, _)| t.chars().count() + 1).sum::<usize>() + 1;
    let field = |g: &mut Grid, line: usize, label: &str, input: &Input, editing: bool, hint: &str, room: usize| {
        g.put(line, left, label, if editing { Role::Accent } else { Role::Muted });
        let x = left + label_w;
        let room = room.max(6);
        let shown = if editing { with_caret(&input.text, input.caret, room) } else if input.text.is_empty() { fit(hint, room) } else { fit(&input.text, room) };
        let role = if !editing && input.text.is_empty() { Role::Muted } else { Role::Title };
        g.put(line, x, &shown, role);
        g.panel(line, x.saturating_sub(1)..x + room + 1);
    };
    let room = width.saturating_sub(label_w + toggles_w + 2);
    field(&mut g, line, "Search", &page.query, page.editing == Some(Field::Query), "what to look for", room);
    let mut x = left + label_w + room + 2;
    for (text, on) in &toggles {
        g.put(line, x, text, if *on { Role::Accent } else { Role::Muted });
        if *on {
            g.underline(line, x..x + text.chars().count());
        }
        x += text.chars().count() + 1;
    }
    line += 1;
    if page.replace_open {
        field(&mut g, line, "Replace", &page.replace, page.editing == Some(Field::Replace), "what it becomes", room);
        let x = left + label_w + room + 2;
        g.put(line, x, "AB→", if page.preserve_case { Role::Accent } else { Role::Muted });
        if page.preserve_case {
            g.underline(line, x..x + 3);
        }
        line += 1;
    }
    // The scope and the files.
    g.put(line, left, "in", Role::Muted);
    let mut x = left + label_w;
    for &scope in &page.scopes {
        let on = scope == page.scope();
        let text = scope.label();
        g.put(line, x, text, if on { Role::Accent } else { Role::Muted });
        if on {
            g.underline(line, x..x + text.chars().count());
        }
        x += text.chars().count() + 2;
    }
    if page.options.hidden {
        x = g.put(line, x, "+hidden", Role::Warn) + 2;
    }
    let globs_room = width.saturating_sub(x - left + 7).max(8);
    let editing_globs = page.editing == Some(Field::Globs);
    if editing_globs || !page.globs.text.is_empty() || width > 70 {
        let at = x;
        g.put(line, at, "files", if editing_globs { Role::Accent } else { Role::Muted });
        let shown = if editing_globs { with_caret(&page.globs.text, page.globs.caret, globs_room) } else if page.globs.text.is_empty() { fit("*.rs !target", globs_room) } else { fit(&page.globs.text, globs_room) };
        let role = if !editing_globs && page.globs.text.is_empty() { Role::Muted } else { Role::Title };
        g.put(line, at + 6, &shown, role);
        if editing_globs || !page.globs.text.is_empty() {
            g.panel(line, at + 5..at + 6 + globs_room + 1);
        }
    }
    line += 1;
    // How it went.
    let (status, role) = status_line(page);
    g.put(line, left, &fit(&status, width), role);
    line += 1;
    g.rule(line, left..left + width);
    line += 1;

    // The results.
    let rows = page.rows();
    let replacing = page.replace_open && page.pattern.is_some();
    for (r, row) in rows.iter().enumerate() {
        let y = line + r;
        let focused = r == page.cursor && page.editing.is_none();
        match *row {
            Row::File(fi) => {
                let file = &page.files[fi];
                let chevron = if file.folded { "▸ " } else { "▾ " };
                let mut x = g.put(y, left, chevron, Role::Muted);
                let name = file.rel.rsplit('/').next().unwrap_or(&file.rel);
                let icon = crate::icon::file_icon(name);
                x = g.put(y, x, &format!("{icon} "), Role::Muted);
                let count = if file.left_out { "left out".to_string() } else if replacing && file.included() != file.matches() { format!("{}/{}", file.included(), file.matches()) } else { file.matches().to_string() };
                let room = width.saturating_sub(x - left + count.chars().count() + 2);
                let shown = fit_path(&file.rel, room);
                let dir_len = shown.rfind('/').map(|i| shown[..=i].chars().count()).unwrap_or(0);
                let dir: String = shown.chars().take(dir_len).collect();
                let base: String = shown.chars().skip(dir_len).collect();
                let x2 = g.put(y, x, &dir, Role::Muted);
                g.put(y, x2, &base, if file.left_out { Role::Muted } else { Role::Title });
                g.put(y, left + width - count.chars().count(), &count, Role::Muted);
            }
            Row::Line(fi, li) => {
                let file = &page.files[fi];
                let l = &file.lines[li];
                let num = l.number.to_string();
                let gutter = 7;
                let context = l.matches.is_empty();
                let out = file.line_left_out(l);
                let current = page.current == Some((fi, li));
                g.put(y, left + gutter - 1 - num.len().min(gutter - 1), &num, if current { Role::Accent } else { Role::Muted });
                let x = left + gutter;
                let room = width.saturating_sub(gutter + if out { 10 } else { 0 });
                let pieces = line_pieces(page, file, l, replacing && !out, room);
                let mut at = x;
                for (text, role, lit) in pieces {
                    let role = if context || out { Role::Muted } else { role };
                    let end = g.put(y, at, &text, role);
                    if lit && !out {
                        g.underline(y, at..end);
                    }
                    at = end;
                }
                if out {
                    g.put(y, left + width - 8, "left out", Role::Muted);
                }
            }
        }
        if focused {
            g.focus(y, left..left + width);
        }
    }
    if rows.is_empty() && !matches!(page.status, Status::Searching { .. }) {
        let hint = match (&page.status, page.query.text.is_empty()) {
            (_, true) => "Type what to look for. Alt-c match case · Alt-w whole word · Alt-r regex · Alt-s scope",
            (Status::Done { .. }, false) => "Nothing found.",
            _ => "",
        };
        g.put(line, left, &fit(hint, width), Role::Muted);
    }
    if page.confirm {
        let (n, files) = page.included();
        g.popup = Some(crate::page::Popup {
            line: 0,
            col: left + label_w,
            rows: vec![
                vec![(format!("Replace {n} match{} in {files} file{}?", if n == 1 { "" } else { "es" }, if files == 1 { "" } else { "s" }), Role::Title)],
                vec![("y ".into(), Role::Accent), ("replace    ".into(), Role::Text), ("n ".into(), Role::Accent), ("not now".into(), Role::Text)],
            ],
        });
    }
    if let Some(h) = height {
        g.reserve(h.saturating_sub(3));
    }
    let keys: Vec<(&str, &str)> = match page.editing {
        Some(_) => vec![("Enter", "results"), ("Tab", "next field"), ("Alt-c", "case"), ("Alt-w", "word"), ("Alt-r", "regex"), ("Alt-s", "scope"), ("↑ ↓", "history"), ("Esc", "stop typing")],
        None if page.replace_open => vec![("x", "leave out"), ("X", "the file"), ("R", "replace all"), ("Alt-p", "keep case"), ("Enter", "open"), ("r", "edit"), ("Esc", "close replace")],
        None => vec![("Enter", "open"), ("p", if page.following { "stop following" } else { "follow" }), ("Tab", "fold"), ("/", "search"), ("r", "replace"), ("f", "files"), ("+ -", "context"), ("q", "close")],
    };
    g.keys(left, width, &keys);
    g.finish()
}

/// The line under the fields: the count, how long, what was skipped --
/// or why the regex won't do.
fn status_line(page: &SearchPage) -> (String, Role) {
    if let Some(error) = &page.error {
        return (format!("✗ {error}"), Role::Bad);
    }
    if let Some(message) = &page.message {
        return (message.clone(), Role::Good);
    }
    let plural = |n: usize, one: &str, many: &str| format!("{} {}", group(n), if n == 1 { one } else { many });
    let found = plural(page.total(), "match", "matches");
    let files = plural(page.files.len(), "file", "files");
    match &page.status {
        Status::Idle => (String::new(), Role::Muted),
        Status::Searching { files: searched } => (format!("searching… {found} in {files} so far · {} searched", plural(*searched, "file", "files")), Role::Muted),
        Status::Failed(why) => (format!("✗ {why}"), Role::Bad),
        Status::Done { ms, binary, truncated, .. } => {
            let mut s = if page.replace_open && page.pattern.is_some() {
                let (n, f) = page.included();
                format!("{} of {found} in {} will change", group(n), plural(f, "file", "files"))
            } else {
                format!("{found} in {files} · {ms} ms")
            };
            if *truncated {
                s.push_str(" · stopped there (search.max_results)");
            }
            if *binary > 0 {
                s.push_str(&format!(" · {} skipped", plural(*binary, "binary file", "binary files")));
            }
            (s, if *truncated { Role::Warn } else { Role::Muted })
        }
    }
}

/// `1,204`.
fn group(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A path in `max` columns, its file name kept: the folders cut first.
fn fit_path(path: &str, max: usize) -> String {
    let n = path.chars().count();
    if n <= max {
        return path.to_string();
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    let name_n = name.chars().count();
    if name_n + 2 >= max {
        return fit(name, max);
    }
    let keep = max - name_n - 1;
    format!("{}…/{name}", path.chars().take(keep.saturating_sub(1)).collect::<String>())
}

/// One result line as pieces (text, role, lit): the matches lit, and
/// while replacing each followed by what it becomes. Cut to `room`
/// columns around the first match.
fn line_pieces(page: &SearchPage, file: &ResultFile, line: &Line, replacing: bool, room: usize) -> Vec<(String, Role, bool)> {
    let text = &line.text;
    let mut pieces: Vec<(String, Role, bool)> = Vec::new();
    let mut at = 0;
    if line.cut.0 {
        pieces.push(("…".into(), Role::Muted, false));
    }
    for (nth, m) in line.matches.iter().enumerate() {
        pieces.push((text[at..m.start].to_string(), Role::Text, false));
        let skipped = file.skipped.contains(&(line.number, nth)) || file.left_out;
        match (&page.pattern, replacing && !skipped) {
            (Some(pattern), true) => {
                pieces.push((text[m.clone()].to_string(), Role::Bad, true));
                let new = pattern.replacement_for(text, m.clone(), &page.replace.text, page.preserve_case);
                pieces.push((new, Role::Good, false));
            }
            _ => pieces.push((text[m.clone()].to_string(), Role::Accent, true)),
        }
        at = m.end;
    }
    pieces.push((text[at..].to_string(), Role::Text, false));
    if line.cut.1 {
        pieces.push(("…".into(), Role::Muted, false));
    }
    // Leading indentation is noise here.
    if let Some(first) = pieces.iter_mut().find(|p| !p.0.is_empty()) {
        if first.1 == Role::Text {
            first.0 = first.0.trim_start().to_string();
        }
    }
    // Too wide: start a little before the first match.
    let total: usize = pieces.iter().map(|p| p.0.chars().count()).sum();
    if total > room {
        let first_lit: usize = pieces.iter().take_while(|p| !p.2).map(|p| p.0.chars().count()).sum();
        let mut skip = first_lit.saturating_sub(room / 4);
        if skip > 0 {
            let mut out = vec![("…".to_string(), Role::Muted, false)];
            for (t, r, l) in pieces {
                let n = t.chars().count();
                if skip >= n {
                    skip -= n;
                    continue;
                }
                out.push((t.chars().skip(skip).collect(), r, l));
                skip = 0;
            }
            pieces = out;
        }
        // And whatever's past the room goes.
        let mut left = room;
        let mut out = Vec::new();
        for (t, r, l) in pieces {
            if left == 0 {
                break;
            }
            let n = t.chars().count();
            if n <= left {
                left -= n;
                out.push((t, r, l));
            } else {
                out.push((fit(&t, left), r, l));
                left = 0;
            }
        }
        pieces = out;
    }
    pieces.into_iter().filter(|p| !p.0.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(path: &str, lines: &[(usize, &str, &str)]) -> FileResult {
        let pattern = |needle: &str, text: &str| text.match_indices(needle).map(|(i, m)| i..i + m.len()).collect();
        FileResult {
            path: PathBuf::from("/p").join(path),
            lines: lines.iter().map(|(n, text, needle)| Line { number: *n, text: text.to_string(), matches: if needle.is_empty() { vec![] } else { pattern(needle, text) }, cut: (false, false) }).collect(),
        }
    }

    fn page() -> SearchPage {
        let mut p = SearchPage::new(PathBuf::from("/p"), vec![ScopeKind::Project, ScopeKind::OpenFiles], ScopeKind::Project);
        p.query = Input::new("helper");
        p.begin(Pattern::new("helper", &Options::default()).ok());
        p.push(vec![found("src/main.rs", &[(1, "fn helper() {", "helper"), (2, "    // context", ""), (7, "    let v = helper();", "helper")]), found("docs/notes.md", &[(3, "The helper helps.", "helper")])], 9);
        p.status = Status::Done { files: 9, ms: 4, binary: 0, truncated: false };
        p
    }

    #[test]
    fn results_are_grouped_by_file_with_counts_and_lit_matches() {
        let p = page();
        let text = layout(&p, 80, Some(30)).text;
        assert!(text.contains("3 matches in 2 files"), "{text}");
        assert!(text.contains("src/main.rs") && text.contains("docs/notes.md"));
        assert!(text.contains("7 let v = helper();"), "indentation trimmed, numbered:\n{text}");
        assert!(text.lines().last().unwrap().contains("Enter open"));
    }

    #[test]
    fn the_first_results_put_the_cursor_on_the_first_match() {
        let p = page();
        assert_eq!(p.rows()[p.cursor], Row::Line(0, 0));
    }

    #[test]
    fn the_cursor_skips_context_and_enter_opens_the_match() {
        let mut p = page();
        p.cursor = 0;
        assert_eq!(p.key(Key::Char('j')), Action::Show { path: PathBuf::from("/p/src/main.rs"), line: 1, col: 4 });
        assert!(matches!(p.key(Key::Char('j')), Action::Show { line: 7, .. }), "the context line is passed");
        assert_eq!(p.key(Key::Enter), Action::Open { path: PathBuf::from("/p/src/main.rs"), line: 7, col: 13 });
    }

    #[test]
    fn typing_searches_soon_and_enter_searches_now() {
        let mut p = page();
        p.key(Key::Char('/'));
        assert!(p.typing());
        assert_eq!(p.key(Key::Char('s')), Action::SearchSoon);
        assert_eq!(p.query.text, "helpers");
        assert_eq!(p.key(Key::Enter), Action::Search);
        assert!(!p.typing());
        assert_eq!(p.toggle('w'), Action::Search);
        assert!(p.options.whole_word);
    }

    #[test]
    fn replace_shows_each_change_and_leaves_out_what_is_marked() {
        let mut p = page();
        p.key(Key::Char('r'));
        for c in "assist".chars() {
            assert_eq!(p.key(Key::Char(c)), Action::None, "the replacement only redraws");
        }
        p.key(Key::Escape);
        let text = layout(&p, 80, Some(30)).text;
        assert!(text.contains("fn helperassist() {"), "old then new:\n{text}");
        assert!(text.contains("3 of 3 matches in 2 files will change"), "{text}");
        // The first match's line is left out, then the second file.
        p.key(Key::Char('j'));
        p.key(Key::Char('x'));
        assert_eq!(p.included(), (2, 2));
        p.cursor = p.rows().iter().position(|&r| r == Row::File(1)).unwrap();
        p.key(Key::Char('x'));
        assert_eq!(p.included(), (1, 1));
        let text = layout(&p, 80, Some(30)).text;
        assert!(text.contains("left out"), "{text}");
        assert_eq!(p.key(Key::Char('R')), Action::None);
        assert!(p.confirm && layout(&p, 80, Some(30)).all_text().contains("Replace 1 match in 1 file?"));
        assert_eq!(p.key(Key::Char('y')), Action::Apply);
    }

    #[test]
    fn folding_and_the_current_match() {
        let mut p = page();
        p.key(Key::Tab);
        assert_eq!(p.rows().len(), 3, "main.rs folded: its file row and notes.md's two");
        p.set_current((0, 2));
        assert!(!p.files[0].folded, "SPC p n opens the file it lands in");
        assert_eq!(p.rows()[p.cursor], Row::Line(0, 2));
        assert_eq!(p.match_rows(), vec![(0, 0), (0, 2), (1, 0)]);
    }

    #[test]
    fn a_bad_regex_and_history() {
        let mut p = page();
        p.error = Some("unclosed group".into());
        assert!(layout(&p, 80, Some(30)).text.contains("✗ unclosed group"));
        p.history = vec!["newest".into(), "older".into()];
        p.edit(Field::Query);
        p.key(Key::Up);
        assert_eq!(p.query.text, "newest");
        p.key(Key::Up);
        assert_eq!(p.query.text, "older");
        p.key(Key::Down);
        assert_eq!(p.query.text, "newest");
    }

    #[test]
    fn scopes_cycle_through_those_offered() {
        let mut p = page();
        assert_eq!(p.toggle('s'), Action::NextScope);
        p.next_scope();
        assert_eq!(p.scope(), ScopeKind::OpenFiles);
        p.next_scope();
        assert_eq!(p.scope(), ScopeKind::Project);
    }

    #[test]
    fn long_paths_keep_their_file_name() {
        assert_eq!(fit_path("crates/fenix-gui/src/app/search_host.rs", 24), "crates/f…/search_host.rs");
        assert_eq!(group(1204), "1,204");
    }
}
