//! The debug drawer (the debugger overhaul's second phase): the dock made
//! somewhere to work. `SPC u f` goes into it and Esc comes back out:
//! `h` / `l` go from column to column, `j` / `k` down and up them, Enter
//! selects a frame -- the values in the code move to it -- or opens a
//! variable, `x` takes a watch or a breakpoint away. Its last line is the
//! evaluate line (`i` in the drawer, `SPC u e` from the code), whose
//! answers go to the console beside the program's output.
//!
//! And the lens: `K` on a name while stopped shows its value in a card,
//! what's in it under it, where `K` otherwise shows the language server's
//! hover.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use fenix_keymap::{KeyCode, KeyPress, NamedKey as FenixNamedKey};

use super::debug_breakpoints::BreakpointPart;
use super::debug_host::{dock_columns, DEBUG_DOCK_ROWS};
use super::*;

/// How many lines the console keeps.
const CONSOLE_LINES: usize = 500;

/// The rows each column shows: the dock but its header, the columns'
/// titles and the evaluate line.
pub(super) const LIST_ROWS: usize = DEBUG_DOCK_ROWS - 3;

/// The most of a value's children the lens's card lists.
const LENS_CHILDREN: usize = 12;

/// The most lines of one value the lens's card shows.
const LENS_VALUE_LINES: usize = 16;

/// `line` at the end of `console`, the oldest gone once it's full.
pub(super) fn console_push(console: &mut VecDeque<String>, line: String) {
    if console.len() == CONSOLE_LINES {
        console.pop_front();
    }
    console.push_back(line);
}

/// A variable in the drawer: what's in it fetched once it's opened.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct DebugVar {
    pub name: String,
    pub value: String,
    /// The adapter's handle on what's in it; 0 for a value with nothing in
    /// it.
    pub reference: i32,
    /// What's in it, once fetched.
    pub children: Option<Vec<DebugVar>>,
}

impl DebugVar {
    fn from_dap(v: fenix_dap::types::Variable) -> DebugVar {
        DebugVar { name: v.name, value: v.value, reference: v.variables_reference, children: None }
    }
}

/// One of the selected frame's scopes (Locals, Globals): its variables
/// once fetched.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct DebugScope {
    pub name: String,
    pub reference: i32,
    pub vars: Option<Vec<DebugVar>>,
}

impl DebugScope {
    /// A scope with `vars`, nothing in them: for tests.
    #[cfg(test)]
    pub fn of(name: &str, vars: &[(&str, &str)]) -> DebugScope {
        DebugScope { name: name.into(), reference: 1, vars: Some(vars.iter().map(|(n, v)| DebugVar { name: n.to_string(), value: v.to_string(), reference: 0, children: None }).collect()) }
    }

    /// Its variables' names and values, what's in them aside.
    pub fn values(&self) -> Vec<(String, String)> {
        self.vars.iter().flatten().map(|v| (v.name.clone(), v.value.clone())).collect()
    }
}

/// Whether the scope or variable at `path` is open: the first scope is
/// unless you closed it, everything else isn't unless you opened it.
pub(super) fn is_open(toggled: &HashSet<Vec<String>>, scopes: &[DebugScope], path: &[String]) -> bool {
    let first_scope = path.len() == 1 && scopes.first().is_some_and(|s| s.name == path[0]);
    first_scope != toggled.contains(path)
}

/// What's under `path` in `scopes`: a scope's variables, or a variable's.
fn under_mut<'a>(scopes: &'a mut [DebugScope], path: &[String]) -> Option<&'a mut Option<Vec<DebugVar>>> {
    let (first, rest) = path.split_first()?;
    let mut at = &mut scopes.iter_mut().find(|s| &s.name == first)?.vars;
    for name in rest {
        at = &mut at.as_mut()?.iter_mut().find(|v| &v.name == name)?.children;
    }
    Some(at)
}

/// The adapter's handle on what's under `path`, and whether it's here yet.
fn reference_at(scopes: &[DebugScope], path: &[String]) -> Option<(i32, bool)> {
    let (first, rest) = path.split_first()?;
    let scope = scopes.iter().find(|s| &s.name == first)?;
    let (mut reference, mut under) = (scope.reference, &scope.vars);
    for name in rest {
        let var = under.as_ref()?.iter().find(|v| &v.name == name)?;
        (reference, under) = (var.reference, &var.children);
    }
    Some((reference, under.is_some()))
}

/// What a row of the drawer does when it's chosen.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum RowAct {
    None,
    /// The `n`th frame, selected.
    Frame(usize),
    /// The scope or variable at this path, opened or closed: when it has
    /// anything in it.
    Var(Vec<String>, bool),
    /// The `n`th watch.
    Watch(usize),
    /// A breakpoint: its file and 1-based line.
    Breakpoint(PathBuf, usize),
    /// An exception the adapter can stop on, by its filter.
    Exception(String),
    /// The `n`th pin.
    Pin(usize),
}

/// The variables column's rows: each scope, and under it, as far as it's
/// open, its variables, a level of indent each.
pub(super) fn variable_rows(scopes: &[DebugScope], toggled: &HashSet<Vec<String>>) -> Vec<(String, RowAct)> {
    fn under(out: &mut Vec<(String, RowAct)>, vars: &Option<Vec<DebugVar>>, path: &mut Vec<String>, depth: usize, scopes: &[DebugScope], toggled: &HashSet<Vec<String>>) {
        let indent = "  ".repeat(depth);
        let Some(vars) = vars else {
            out.push((format!("{indent}  …"), RowAct::None));
            return;
        };
        for var in vars {
            path.push(var.name.clone());
            let has = var.reference > 0;
            let open = has && is_open(toggled, scopes, path);
            let mark = match (has, open) {
                (false, _) => " ",
                (true, false) => "▸",
                (true, true) => "▾",
            };
            let value: String = var.value.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
            out.push((format!("{indent}{mark} {} = {value}", var.name), RowAct::Var(path.clone(), has)));
            if open {
                under(out, &var.children, path, depth + 1, scopes, toggled);
            }
            path.pop();
        }
    }
    let mut out = Vec::new();
    for scope in scopes {
        let mut path = vec![scope.name.clone()];
        let open = is_open(toggled, scopes, &path);
        out.push((format!("{} {}", if open { "▾" } else { "▸" }, scope.name), RowAct::Var(path.clone(), true)));
        if open {
            under(&mut out, &scope.vars, &mut path, 1, scopes, toggled);
        }
    }
    out
}

/// The expression `K` looks at in `line` at character `col`: the name
/// there, and the names before it it's an attribute of (`self.rows`).
/// None off a name, or on a number.
pub(super) fn lens_expression(line: &str, col: usize) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let word = |c: char| c.is_alphanumeric() || c == '_';
    if !chars.get(col).copied().is_some_and(word) {
        return None;
    }
    let (mut start, mut end) = (col, col);
    while start > 0 && word(chars[start - 1]) {
        start -= 1;
    }
    while end + 1 < chars.len() && word(chars[end + 1]) {
        end += 1;
    }
    // Back over `name.` before it, as long as there's a name.
    while start >= 2 && chars[start - 1] == '.' && word(chars[start - 2]) {
        start -= 1;
        while start > 0 && word(chars[start - 1]) {
            start -= 1;
        }
    }
    let expression: String = chars[start..=end].iter().collect();
    (!expression.starts_with(|c: char| c.is_ascii_digit())).then_some(expression)
}

/// The lens's card: the expression and its value, its type under them.
pub(super) fn lens_card(expression: &str, value: &str, type_: Option<&str>) -> String {
    let mut lines: Vec<String> = value.lines().take(LENS_VALUE_LINES).map(str::to_string).collect();
    if lines.is_empty() {
        lines.push(String::new());
    }
    let mut out = format!("{expression} = {}", lines[0]);
    for line in &lines[1..] {
        out.push('\n');
        out.push_str(line);
    }
    if value.lines().count() > LENS_VALUE_LINES {
        out.push_str("\n…");
    }
    if let Some(t) = type_.filter(|t| !t.is_empty()) {
        out.push_str(&format!("\n{t}"));
    }
    out
}

/// `card` with what's in its value listed under it.
pub(super) fn lens_card_with(card: &str, children: &[DebugVar]) -> String {
    let mut out = card.to_string();
    for var in children.iter().take(LENS_CHILDREN) {
        let value: String = var.value.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        out.push_str(&format!("\n  {} = {value}", var.name));
    }
    if children.len() > LENS_CHILDREN {
        out.push_str(&format!("\n  … {} more in the drawer (SPC u f)", children.len() - LENS_CHILDREN));
    }
    out
}

/// The drawer's columns, left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Column {
    Stack,
    Variables,
    Watches,
    Breakpoints,
    Console,
}

const COLUMNS: [Column; 5] = [Column::Stack, Column::Variables, Column::Watches, Column::Breakpoints, Column::Console];

impl Column {
    fn title(self) -> &'static str {
        match self {
            Column::Stack => "CALL STACK",
            Column::Variables => "VARIABLES",
            Column::Watches => "WATCHES",
            Column::Breakpoints => "BREAKPOINTS",
            Column::Console => "CONSOLE",
        }
    }

    /// Its share of the drawer's width.
    fn share(self) -> usize {
        match self {
            Column::Stack => 3,
            Column::Variables => 4,
            Column::Watches | Column::Breakpoints => 2,
            Column::Console => 4,
        }
    }
}

/// The evaluate line, being typed.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct EvalLine {
    pub text: String,
    pub purpose: LinePurpose,
}

/// What the evaluate line's for.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) enum LinePurpose {
    /// Asked once, the answer to the console.
    #[default]
    Evaluate,
    /// A watch added.
    Watch,
    /// A breakpoint's condition, hit count or message: its file and line.
    Breakpoint(PathBuf, usize, BreakpointPart),
}

impl LinePurpose {
    fn prompt(&self) -> &'static str {
        match self {
            LinePurpose::Evaluate => "› ",
            LinePurpose::Watch => "watch › ",
            LinePurpose::Breakpoint(_, _, part) => part.prompt(),
        }
    }
}

/// The breakpoints' column, which `SPC u x` goes to.
pub(super) const BREAKPOINTS_COLUMN: usize = 3;

/// The drawer's own state, beside the session's: whether it has the keys,
/// which column, each column's selected row and first row shown, and the
/// evaluate line.
#[derive(Debug, Default)]
pub(super) struct DebugDrawer {
    pub focused: bool,
    pub column: usize,
    pub selected: [usize; 5],
    pub scroll: [usize; 5],
    pub line: Option<EvalLine>,
    /// What the evaluate line has been asked, the newest last.
    pub history: Vec<String>,
    /// Where Up has gone back to in `history`.
    pub recalled: Option<usize>,
}

/// `scroll`, moved as little as keeps row `selected` of `len` among the
/// `rows` shown.
pub(super) fn keep_shown(selected: usize, scroll: usize, len: usize, rows: usize) -> usize {
    let scroll = scroll.min(len.saturating_sub(rows));
    if selected < scroll {
        selected
    } else if selected >= scroll + rows {
        selected + 1 - rows
    } else {
        scroll
    }
}

impl App {
    /// Whether the drawer has the keys: it's out, and was gone into.
    pub(super) fn debug_drawer_focused(&self) -> bool {
        self.debug_drawer.focused && self.debug_dock_shown()
    }

    /// `SPC u f`: into the drawer, brought out first; out of it when in.
    pub(crate) fn debug_focus_drawer(&mut self) {
        if self.debug_session.is_none() {
            self.set_error("no debug session -- SPC u u or F5 starts one".to_string());
            return;
        }
        if self.debug_drawer_focused() {
            self.debug_drawer.focused = false;
            self.debug_drawer.line = None;
            return;
        }
        self.debug_dock_open = true;
        self.debug_drawer.focused = true;
    }

    /// `SPC u e`: the evaluate line, in the drawer.
    pub(crate) fn debug_evaluate(&mut self) {
        if self.debug_session.is_none() {
            self.set_error("no debug session -- SPC u u or F5 starts one".to_string());
            return;
        }
        self.debug_dock_open = true;
        self.debug_drawer.focused = true;
        self.debug_drawer.line = Some(EvalLine::default());
        self.debug_drawer.recalled = None;
    }

    /// The rows of `column`, and what each does.
    pub(super) fn debug_drawer_rows(&self, column: Column) -> Vec<(String, RowAct)> {
        let Some(session) = self.debug_session.as_ref() else { return Vec::new() };
        let plain = |text: String| -> Vec<(String, RowAct)> { text.lines().map(|l| (l.to_string(), RowAct::None)).collect() };
        match column {
            Column::Stack if session.ended || (session.stopped_thread.is_none() && session.children.is_empty()) => plain(render_call_stack_text(session)),
            Column::Stack => stack_rows(session),
            Column::Variables if session.variables.is_empty() => plain(if session.stopped_thread.is_some() { "…".into() } else { "(not stopped)".into() }),
            Column::Variables => variable_rows(&session.variables, &session.toggled),
            Column::Watches if session.watches.is_empty() && self.debug_pins.is_empty() => plain("(none -- a adds one, SPC u P pins one)".into()),
            Column::Watches => {
                let mut rows: Vec<(String, RowAct)> = session
                    .watches
                    .iter()
                    .enumerate()
                    .map(|(i, expr)| (format!("{expr} = {}", session.watch_values.get(expr).map(String::as_str).unwrap_or("…")), RowAct::Watch(i)))
                    .collect();
                rows.extend(self.debug_pins.iter().enumerate().map(|(i, pin)| {
                    let value = session.pin_values.get(pin).map(|(v, _)| v.as_str()).unwrap_or("…");
                    (format!("◆ {} = {value}  {}:{}", pin.expression, file_label(&pin.path), pin.line), RowAct::Pin(i))
                }));
                rows
            }
            Column::Breakpoints => {
                // The exceptions it can stop on, then the breakpoints.
                let mut rows: Vec<(String, RowAct)> = session
                    .exception_filters
                    .iter()
                    .map(|f| (format!("{} {}", if self.debug_exception_on(f) { "☑" } else { "☐" }, f.label), RowAct::Exception(f.filter.clone())))
                    .collect();
                let mut all: Vec<(PathBuf, usize, String)> = self
                    .breakpoints
                    .iter()
                    .flat_map(|(path, lines)| lines.iter().map(move |(line, b)| (path.clone(), *line, format!("{} {}:{line}  {}", b.glyph(), file_label(path), b.describe()).trim_end().to_string())))
                    .collect();
                all.sort_by_key(|(path, line, _)| (file_label(path), *line));
                if all.is_empty() {
                    rows.push(("(none -- F9 or SPC u b)".into(), RowAct::None));
                }
                rows.extend(all.into_iter().map(|(path, line, text)| (text, RowAct::Breakpoint(path, line))));
                rows
            }
            Column::Console if session.console.is_empty() => plain("(the program's output, and i's answers)".into()),
            Column::Console => session.console.iter().map(|l| (l.clone(), RowAct::None)).collect(),
        }
    }

    /// Column `c`'s first row shown: kept where the selection is while
    /// it's the drawer's column; the console's newest lines otherwise.
    fn debug_drawer_first_row(&self, c: usize, len: usize) -> usize {
        let drawer = &self.debug_drawer;
        let followed = drawer.focused && drawer.column == c;
        if COLUMNS[c] == Column::Console && !followed {
            return len.saturating_sub(LIST_ROWS);
        }
        keep_shown(drawer.selected[c].min(len.saturating_sub(1)), drawer.scroll[c], len, LIST_ROWS)
    }

    /// The dock's text, `width` characters across: a header with the
    /// session and where it is, the columns side by side, and the
    /// evaluate line.
    pub(super) fn debug_dock_spans(&self, width: usize) -> Vec<(String, glyphon::Color, bool)> {
        let Some(session) = self.debug_session.as_ref() else { return Vec::new() };
        let theme = &self.theme;
        let (accent, muted) = (theme.git_modified, theme.gutter_fg);
        let focused = self.debug_drawer_focused();
        let mut columns: Vec<(String, Vec<String>, usize)> = Vec::new();
        for (c, column) in COLUMNS.iter().enumerate() {
            let rows = self.debug_drawer_rows(*column);
            let first = self.debug_drawer_first_row(c, rows.len());
            let mut title = column.title().to_string();
            if rows.len() > LIST_ROWS {
                title.push_str(&format!("  {}-{}/{}", first + 1, (first + LIST_ROWS).min(rows.len()), rows.len()));
            }
            columns.push((title, rows.into_iter().skip(first).take(LIST_ROWS).map(|(text, _)| text).collect(), column.share()));
        }
        let borrowed: Vec<(&str, Vec<String>, usize)> = columns.iter().map(|(t, l, s)| (t.as_str(), l.clone(), *s)).collect();
        let rows = dock_columns(&borrowed, width, LIST_ROWS + 1);
        let mut out: Vec<(String, glyphon::Color, bool)> = Vec::new();
        out.push((format!("{}  ", file_label(Path::new(&session.name))), theme.fg, false));
        out.push((debug_host::debug_state_text(session), if session.stopped_thread.is_some() { accent } else { muted }, false));
        let keys = if focused {
            "    j k move  h l column  Enter open  x remove  a watch  i evaluate  Esc back\n"
        } else if session.ended {
            "    F5 starts another  SPC u d hides\n"
        } else {
            "    F5 continue  F10 next  F11 in  ⇧F11 out  ⇧F5 stop  SPC u f drawer\n"
        };
        out.push((keys.to_string(), muted, true));
        for (r, cells) in rows.into_iter().enumerate() {
            for (c, cell) in cells.into_iter().enumerate() {
                if c > 0 {
                    out.push((" │ ".to_string(), muted, false));
                }
                let mine = focused && self.debug_drawer.column == c;
                let color = match r {
                    0 if mine => accent,
                    0 => muted,
                    _ if COLUMNS[c] == Column::Stack && cell.trim_start().starts_with('▸') && session.stopped_thread.is_some() => accent,
                    _ => theme.fg,
                };
                out.push((cell, color, false));
            }
            out.push(("\n".to_string(), theme.fg, false));
        }
        match &self.debug_drawer.line {
            Some(line) if focused => {
                out.push((line.purpose.prompt().to_string(), accent, false));
                out.push((format!("{}▏", line.text), theme.fg, false));
            }
            _ if focused && COLUMNS[self.debug_drawer.column] == Column::Breakpoints => out.push(("t on/off  e condition  H hit count  L log message  x remove".to_string(), muted, true)),
            _ if focused => out.push(("i evaluates in the selected frame, a adds a watch".to_string(), muted, true)),
            _ => out.push(("SPC u e evaluates  K on a name shows its value".to_string(), muted, true)),
        }
        out
    }

    /// Where the drawer's selection is drawn, `width` characters across:
    /// its row in the dock (the header row 0), its first column and its
    /// width in characters. None while the drawer hasn't the keys.
    pub(super) fn debug_drawer_highlight(&self, width: usize) -> Option<(usize, usize, usize)> {
        if !self.debug_drawer_focused() || self.debug_drawer.line.is_some() {
            return None;
        }
        let c = self.debug_drawer.column;
        let len = self.debug_drawer_rows(COLUMNS[c]).len();
        if len == 0 {
            return None;
        }
        let first = self.debug_drawer_first_row(c, len);
        let selected = self.debug_drawer.selected[c].min(len - 1);
        let total: usize = COLUMNS.iter().map(|c| c.share()).sum();
        let usable = width.saturating_sub((COLUMNS.len() - 1) * 3);
        let widths: Vec<usize> = COLUMNS.iter().map(|c| usable * c.share() / total).collect();
        let x = widths[..c].iter().sum::<usize>() + c * 3;
        Some((2 + selected - first, x, widths[c]))
    }

    /// A key while the drawer has the keys: true when it's the drawer's.
    /// SPC isn't, so the leader works from in it.
    pub(super) fn debug_drawer_key(&mut self, key: KeyPress) -> bool {
        if self.debug_drawer.line.is_some() {
            self.debug_line_key(key);
            return true;
        }
        // SPC is the leader's, and Ctrl's chords (Ctrl-s) are everyone's
        // but the two that page.
        if key == KeyPress::char(' ') || (key.mods.ctrl && !matches!(key.code, KeyCode::Char('d' | 'u'))) {
            return false;
        }
        let c = self.debug_drawer.column;
        let rows = self.debug_drawer_rows(COLUMNS[c]);
        let len = rows.len();
        let selected = self.debug_drawer.selected[c].min(len.saturating_sub(1));
        let step = |to: usize, drawer: &mut DebugDrawer| {
            drawer.selected[c] = to;
            drawer.scroll[c] = keep_shown(to, drawer.scroll[c], len, LIST_ROWS);
        };
        let ctrl = key.mods.ctrl;
        match key.code {
            KeyCode::Named(FenixNamedKey::Escape) | KeyCode::Char('q') => {
                self.debug_drawer.focused = false;
            }
            KeyCode::Char('h') | KeyCode::Named(FenixNamedKey::Left) => self.debug_drawer_column(c.saturating_sub(1)),
            KeyCode::Char('l') | KeyCode::Named(FenixNamedKey::Right) | KeyCode::Named(FenixNamedKey::Tab) => self.debug_drawer_column((c + 1).min(COLUMNS.len() - 1)),
            KeyCode::Char('j') | KeyCode::Named(FenixNamedKey::Down) => step((selected + 1).min(len.saturating_sub(1)), &mut self.debug_drawer),
            KeyCode::Char('k') | KeyCode::Named(FenixNamedKey::Up) => step(selected.saturating_sub(1), &mut self.debug_drawer),
            KeyCode::Char('d') if ctrl => step((selected + LIST_ROWS / 2).min(len.saturating_sub(1)), &mut self.debug_drawer),
            KeyCode::Char('u') if ctrl => step(selected.saturating_sub(LIST_ROWS / 2), &mut self.debug_drawer),
            KeyCode::Char('g') => step(0, &mut self.debug_drawer),
            KeyCode::Char('G') => step(len.saturating_sub(1), &mut self.debug_drawer),
            KeyCode::Named(FenixNamedKey::Enter) => {
                if let Some((_, act)) = rows.into_iter().nth(selected) {
                    self.debug_drawer_open(act);
                }
            }
            KeyCode::Char('x') | KeyCode::Char('d') => {
                if let Some((_, act)) = rows.into_iter().nth(selected) {
                    self.debug_drawer_remove(act);
                }
            }
            KeyCode::Char('a') => self.debug_drawer.line = Some(EvalLine { text: String::new(), purpose: LinePurpose::Watch }),
            // A breakpoint's on/off, condition, hit count and message; an
            // exception's on/off.
            KeyCode::Char('t') | KeyCode::Char('e') | KeyCode::Char('H') | KeyCode::Char('L') => {
                let Some((_, act)) = rows.into_iter().nth(selected) else { return true };
                match (act, key.code) {
                    (RowAct::Breakpoint(path, line), KeyCode::Char('t')) => self.debug_breakpoint_on_off(path, line),
                    (RowAct::Breakpoint(path, line), KeyCode::Char('e')) => self.debug_edit_breakpoint_at(path, line, BreakpointPart::Condition),
                    (RowAct::Breakpoint(path, line), KeyCode::Char('H')) => self.debug_edit_breakpoint_at(path, line, BreakpointPart::Hits),
                    (RowAct::Breakpoint(path, line), KeyCode::Char('L')) => self.debug_edit_breakpoint_at(path, line, BreakpointPart::Log),
                    (RowAct::Exception(filter), KeyCode::Char('t')) => self.debug_toggle_exception(filter),
                    _ => {}
                }
            }
            KeyCode::Char('i') | KeyCode::Char(':') => {
                self.debug_drawer.line = Some(EvalLine::default());
                self.debug_drawer.recalled = None;
            }
            // DEBUG's stepping keys, from in the drawer too.
            KeyCode::Char('n') => self.debug_step_over(),
            KeyCode::Char('s') => self.debug_step_into(),
            KeyCode::Char('o') => self.debug_step_out(),
            KeyCode::Char('c') => self.debug_start_or_continue(),
            _ => {}
        }
        true
    }

    /// To column `c`, its selection where it was left; into the console
    /// at its newest line.
    fn debug_drawer_column(&mut self, c: usize) {
        if COLUMNS[c] == Column::Console && self.debug_drawer.column != c {
            let len = self.debug_drawer_rows(Column::Console).len();
            self.debug_drawer.selected[c] = len.saturating_sub(1);
            self.debug_drawer.scroll[c] = len.saturating_sub(LIST_ROWS);
        }
        self.debug_drawer.column = c;
    }

    /// Enter on a row: a frame selected, a variable opened or closed, a
    /// breakpoint gone to.
    fn debug_drawer_open(&mut self, act: RowAct) {
        match act {
            RowAct::Frame(i) => self.debug_select_frame(i),
            RowAct::Var(path, true) => self.debug_toggle_var(path),
            RowAct::Breakpoint(path, line) => {
                self.debug_drawer.focused = false;
                self.goto_debug_location(&path, line);
            }
            RowAct::Pin(i) => {
                if let Some(pin) = self.debug_pins.get(i).cloned() {
                    self.debug_drawer.focused = false;
                    self.goto_debug_location(&pin.path, pin.line);
                }
            }
            RowAct::Exception(filter) => self.debug_toggle_exception(filter),
            RowAct::Var(_, false) | RowAct::Watch(_) | RowAct::None => {}
        }
    }

    /// `x` on a row: a watch or a breakpoint gone.
    fn debug_drawer_remove(&mut self, act: RowAct) {
        match act {
            RowAct::Watch(i) => {
                let Some(session) = self.debug_session.as_mut() else { return };
                if i < session.watches.len() {
                    let gone = session.watches.remove(i);
                    session.watch_values.remove(&gone);
                }
            }
            RowAct::Breakpoint(path, line) => {
                if let Some(lines) = self.breakpoints.get_mut(&path) {
                    lines.remove(&line);
                }
                self.send_set_breakpoints_for(&path);
                self.refresh_debug_panes();
            }
            RowAct::Pin(i) => {
                if i < self.debug_pins.len() {
                    self.debug_pins.remove(i);
                }
                self.refresh_debug_panes();
            }
            _ => {}
        }
    }

    /// The `i`th frame selected: its values written in the code and
    /// listed in the drawer, and the code shown where it is.
    pub(super) fn debug_select_frame(&mut self, i: usize) {
        let Some(session) = self.debug_session.as_mut() else { return };
        let Some(&frame) = session.frame_ids.get(i) else { return };
        if session.selected_frame == i {
            return;
        }
        session.selected_frame = i;
        session.current_frame_id = Some(frame);
        session.variables.clear();
        let location = session.call_stack.get(i).filter(|(_, path, line)| !path.as_os_str().is_empty() && *line > 0).map(|(_, path, line)| (path.clone(), *line));
        if let Some((path, line)) = location {
            self.goto_debug_location(&path, line);
        }
        self.refresh_debug_panes();
        self.debug_ask_frame();
    }

    /// The selected frame's scopes asked for, and the watches in it.
    pub(super) fn debug_ask_frame(&mut self) {
        let Some(session) = self.debug_session.as_mut() else { return };
        let Some(frame) = session.current_frame_id else { return };
        let on = session.stopped_on;
        let args = fenix_dap::requests::ScopesRequestArguments::builder().frame_id(frame).build();
        session.send(on, fenix_dap::requests::Request::Scopes(args), PendingDapRequest::Scopes { frame });
        for expr in session.watches.clone() {
            let args = fenix_dap::requests::EvaluateRequestArguments::builder().expression(expr.clone()).frame_id(Some(frame)).context(Some(fenix_dap::requests::EvaluateRequestContext::Watch)).build();
            session.send(on, fenix_dap::requests::Request::Evaluate(args), PendingDapRequest::Evaluate { expression: expr });
        }
        for pin in self.debug_pins.clone() {
            self.debug_ask_pin(&pin);
        }
    }

    /// The variables under `path`, asked for in frame `frame`.
    fn debug_ask_vars(&mut self, frame: i32, path: Vec<String>, reference: i32) {
        let Some(session) = self.debug_session.as_mut() else { return };
        let on = session.stopped_on;
        let args = fenix_dap::requests::VariablesRequestArguments::builder().variables_reference(reference).build();
        session.send(on, fenix_dap::requests::Request::Variables(args), PendingDapRequest::Variables { frame, path });
    }

    /// Frame `frame`'s scopes, from connection `on`: each open one's
    /// variables asked for. Late ones, for a frame no longer selected, are
    /// dropped.
    pub(super) fn debug_scopes_came(&mut self, on: u64, frame: i32, scopes: Vec<fenix_dap::types::Scope>) {
        let Some(session) = self.debug_session.as_mut() else { return };
        if session.stopped_on != on || session.current_frame_id != Some(frame) {
            return;
        }
        session.variables = scopes.into_iter().map(|s| DebugScope { name: s.name, reference: s.variables_reference, vars: None }).collect();
        let open: Vec<(Vec<String>, i32)> = session.variables.iter().map(|s| (vec![s.name.clone()], s.reference)).filter(|(path, _)| is_open(&session.toggled, &session.variables, path)).collect();
        for (path, reference) in open {
            self.debug_ask_vars(frame, path, reference);
        }
        self.refresh_debug_panes();
    }

    /// The variables under `path` in frame `frame`, come: put there, and
    /// those among them that are open asked for in turn.
    pub(super) fn debug_variables_came(&mut self, on: u64, frame: i32, path: Vec<String>, vars: Vec<fenix_dap::types::Variable>) {
        let Some(session) = self.debug_session.as_mut() else { return };
        if session.stopped_on != on || session.current_frame_id != Some(frame) {
            return;
        }
        let vars: Vec<DebugVar> = vars.into_iter().map(DebugVar::from_dap).collect();
        let open: Vec<(Vec<String>, i32)> = vars
            .iter()
            .filter(|v| v.reference > 0)
            .map(|v| ([path.clone(), vec![v.name.clone()]].concat(), v.reference))
            .filter(|(p, _)| is_open(&session.toggled, &session.variables, p))
            .collect();
        // The top frame's first scope: the trail's newest stop's values.
        let first_of_top = session.frame_ids.first() == Some(&frame) && path.len() == 1 && session.variables.first().is_some_and(|s| s.name == path[0]);
        let Some(slot) = under_mut(&mut session.variables, &path) else { return };
        *slot = Some(vars);
        if first_of_top {
            let values = session.variables[0].values();
            self.debug_trail_values(values);
        }
        for (path, reference) in open {
            self.debug_ask_vars(frame, path, reference);
        }
        self.refresh_debug_panes();
    }

    /// The scope or variable at `path` opened, what's in it asked for the
    /// first time; or closed.
    pub(super) fn debug_toggle_var(&mut self, path: Vec<String>) {
        let Some(session) = self.debug_session.as_mut() else { return };
        if !session.toggled.remove(&path) {
            session.toggled.insert(path.clone());
        }
        let open = is_open(&session.toggled, &session.variables, &path);
        let (Some(frame), Some((reference, here))) = (session.current_frame_id, reference_at(&session.variables, &path)) else { return };
        if open && !here && reference > 0 {
            self.debug_ask_vars(frame, path, reference);
        }
    }

    /// A key on the evaluate line: typed, gone back over, asked (Enter),
    /// or let go (Esc). Up and Down go through what it's been asked.
    fn debug_line_key(&mut self, key: KeyPress) {
        let Some(line) = self.debug_drawer.line.as_mut() else { return };
        match key.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.debug_drawer.line = None,
            KeyCode::Named(FenixNamedKey::Backspace) => {
                line.text.pop();
            }
            KeyCode::Char('u') if key.mods.ctrl => line.text.clear(),
            KeyCode::Named(FenixNamedKey::Up) | KeyCode::Named(FenixNamedKey::Down) => {
                let history = &self.debug_drawer.history;
                if history.is_empty() {
                    return;
                }
                let up = key.code == KeyCode::Named(FenixNamedKey::Up);
                let at = match (self.debug_drawer.recalled, up) {
                    (None, true) => Some(history.len() - 1),
                    (None, false) => None,
                    (Some(i), true) => Some(i.saturating_sub(1)),
                    (Some(i), false) => (i + 1 < history.len()).then_some(i + 1),
                };
                line.text = at.map(|i| history[i].clone()).unwrap_or_default();
                self.debug_drawer.recalled = at;
            }
            KeyCode::Named(FenixNamedKey::Enter) => {
                let line = self.debug_drawer.line.take().unwrap_or_default();
                let text = line.text.trim().to_string();
                // A breakpoint's part: emptied, it goes.
                if let LinePurpose::Breakpoint(path, at, part) = line.purpose.clone() {
                    self.debug_set_breakpoint_part(path, at, part, text);
                    return;
                }
                if text.is_empty() {
                    return;
                }
                match line.purpose {
                    LinePurpose::Watch => self.debug_watch(text),
                    LinePurpose::Evaluate => {
                        self.debug_drawer.history.retain(|h| *h != text);
                        self.debug_drawer.history.push(text.clone());
                        self.debug_drawer.recalled = None;
                        self.debug_repl(text);
                    }
                    LinePurpose::Breakpoint(..) => {}
                }
            }
            KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => line.text.push(c),
            _ => {}
        }
    }

    /// `expression` asked of the adapter in the selected frame, the
    /// question and the answer to the console.
    pub(super) fn debug_repl(&mut self, expression: String) {
        let Some(session) = self.debug_session.as_mut().filter(|s| !s.ended) else {
            self.set_error("no debug session running".to_string());
            return;
        };
        console_push(&mut session.console, format!("› {expression}"));
        let on = session.stopped_on;
        let args = fenix_dap::requests::EvaluateRequestArguments::builder().expression(expression.clone()).frame_id(session.current_frame_id).context(Some(fenix_dap::requests::EvaluateRequestContext::REPL)).build();
        if !session.send(on, fenix_dap::requests::Request::Evaluate(args), PendingDapRequest::Repl { expression }) {
            console_push(&mut session.console, "  ! the adapter isn't there".to_string());
        }
        self.debug_console_follow();
    }

    /// The evaluate line's answer, to the console.
    pub(super) fn debug_repl_answered(&mut self, _expression: String, result: Result<fenix_dap::responses::SuccessResponse, fenix_dap::responses::ErrorResponse>) {
        let Some(session) = self.debug_session.as_mut() else { return };
        match result {
            Ok(fenix_dap::responses::SuccessResponse::Evaluate(body)) => {
                for line in body.result.lines() {
                    console_push(&mut session.console, format!("  {line}"));
                }
            }
            Ok(_) => {}
            Err(err) => console_push(&mut session.console, format!("  ! {}", err.message.trim())),
        }
        self.debug_console_follow();
    }

    /// The console kept at its newest line while it's the drawer's column.
    fn debug_console_follow(&mut self) {
        let c = COLUMNS.iter().position(|c| *c == Column::Console).unwrap_or(0);
        let len = self.debug_drawer_rows(Column::Console).len();
        self.debug_drawer.selected[c] = len.saturating_sub(1);
        self.debug_drawer.scroll[c] = len.saturating_sub(LIST_ROWS);
    }

    /// `expression` watched, and its value asked for if it's stopped.
    pub(super) fn debug_watch(&mut self, expression: String) {
        let Some(session) = self.debug_session.as_mut() else {
            self.set_error("no debug session running".to_string());
            return;
        };
        if session.watches.contains(&expression) {
            return;
        }
        session.watches.push(expression.clone());
        if let Some(frame) = session.current_frame_id {
            let args = fenix_dap::requests::EvaluateRequestArguments::builder().expression(expression.clone()).frame_id(Some(frame)).context(Some(fenix_dap::requests::EvaluateRequestContext::Watch)).build();
            let on = session.stopped_on;
            session.send(on, fenix_dap::requests::Request::Evaluate(args), PendingDapRequest::Evaluate { expression });
        }
        self.refresh_debug_panes();
    }

    /// `K` while stopped in the project you're in, on a name: its value
    /// asked for, for the lens's card. False when it isn't, and `K` is the
    /// language server's hover.
    pub(super) fn debug_lens(&mut self) -> bool {
        let root = self.integration_root();
        let Some(session) = self.debug_session.as_ref() else { return false };
        if session.ended || session.stopped_thread.is_none() || session.root != root {
            return false;
        }
        let Some(frame) = session.current_frame_id else { return false };
        let (line, col) = self.open().buffer.line_col(&self.cursor());
        let text = self.open().buffer.line(line).to_string();
        let Some(expression) = lens_expression(&text, col) else { return false };
        let Some(session) = self.debug_session.as_mut() else { return false };
        let on = session.stopped_on;
        let args = fenix_dap::requests::EvaluateRequestArguments::builder().expression(expression.clone()).frame_id(Some(frame)).context(Some(fenix_dap::requests::EvaluateRequestContext::Hover)).build();
        session.send(on, fenix_dap::requests::Request::Evaluate(args), PendingDapRequest::Lens { expression })
    }

    /// The lens's value come: its card shown, and what's in it asked for.
    /// One the adapter can't give is the language server's hover instead.
    pub(super) fn debug_lens_answered(&mut self, on: u64, expression: String, result: Result<fenix_dap::responses::SuccessResponse, fenix_dap::responses::ErrorResponse>) {
        let Ok(fenix_dap::responses::SuccessResponse::Evaluate(body)) = result else {
            self.request_hover();
            return;
        };
        let card = lens_card(&expression, &body.result, body.type_.as_deref());
        self.lsp_hover = Some(card.clone());
        self.lsp_hover_fanout = None;
        if body.variables_reference > 0 {
            let Some(session) = self.debug_session.as_mut() else { return };
            let args = fenix_dap::requests::VariablesRequestArguments::builder().variables_reference(body.variables_reference).build();
            session.send(on, fenix_dap::requests::Request::Variables(args), PendingDapRequest::LensChildren { card });
        }
    }

    /// What's in the lens's value, listed on its card -- if it's still up.
    pub(super) fn debug_lens_children(&mut self, card: String, vars: Vec<fenix_dap::types::Variable>) {
        if self.lsp_hover.as_deref() != Some(card.as_str()) {
            return;
        }
        let vars: Vec<DebugVar> = vars.into_iter().map(DebugVar::from_dap).collect();
        self.lsp_hover = Some(lens_card_with(&card, &vars));
    }
}

/// The stack column's rows: the stopped frames, each selectable; with
/// child sessions (js-debug runs each program in one), each session under
/// the one that asked for it, paused or running, the frames under the one
/// that's stopped.
fn stack_rows(session: &DebugSession) -> Vec<(String, RowAct)> {
    let stopped = session.stopped_thread.is_some();
    let frames = |depth: usize| -> Vec<(String, RowAct)> {
        let indent = "  ".repeat(depth);
        session
            .call_stack
            .iter()
            .enumerate()
            .map(|(i, (name, path, line))| {
                let mark = if i == session.selected_frame { "▸" } else { " " };
                let text = if path.as_os_str().is_empty() { format!("{indent}{mark} {name}") } else { format!("{indent}{mark} {name}  {}:{line}", file_label(path)) };
                (text, RowAct::Frame(i))
            })
            .collect()
    };
    if session.children.is_empty() {
        return frames(0);
    }
    let state = |on: u64| if stopped && session.stopped_on == on { "paused" } else { "running" };
    fn under(out: &mut Vec<(String, RowAct)>, session: &DebugSession, parent: u64, depth: usize, state: &dyn Fn(u64) -> &'static str, frames: &dyn Fn(usize) -> Vec<(String, RowAct)>) {
        for child in session.children.iter().filter(|c| c.parent == parent) {
            out.push((format!("{}{} ({})", "  ".repeat(depth), child.name, state(child.generation)), RowAct::None));
            if session.stopped_thread.is_some() && session.stopped_on == child.generation {
                out.extend(frames(depth + 1));
            }
            under(out, session, child.generation, depth + 1, state, frames);
        }
    }
    let mut out = vec![(format!("{} ({})", session.name, state(session.generation)), RowAct::None)];
    if stopped && session.stopped_on == session.generation {
        out.extend(frames(1));
    }
    under(&mut out, session, session.generation, 1, &state, &frames);
    out
}

/// `path`'s file name alone, for a narrow column.
fn file_label(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str, value: &str, reference: i32) -> DebugVar {
        DebugVar { name: name.into(), value: value.into(), reference, children: None }
    }

    #[test]
    fn the_first_scope_is_open_and_the_rest_and_every_variable_closed_until_toggled() {
        let scopes = vec![DebugScope { name: "Locals".into(), reference: 1, vars: Some(vec![var("o", "Order(1)", 7), var("n", "3", 0)]) }, DebugScope { name: "Globals".into(), reference: 2, vars: None }];
        let mut toggled = HashSet::new();
        let rows: Vec<String> = variable_rows(&scopes, &toggled).into_iter().map(|(t, _)| t).collect();
        assert_eq!(rows, ["▾ Locals", "  ▸ o = Order(1)", "    n = 3", "▸ Globals"]);
        toggled.insert(vec!["Locals".to_string(), "o".to_string()]);
        toggled.insert(vec!["Locals".to_string()]);
        assert_eq!(variable_rows(&scopes, &toggled).len(), 2, "Locals closed");
        toggled.remove(&vec!["Locals".to_string()]);
        let rows: Vec<(String, RowAct)> = variable_rows(&scopes, &toggled);
        assert_eq!(rows[1].0, "  ▾ o = Order(1)");
        assert_eq!(rows[2], ("      …".to_string(), RowAct::None), "open, what's in it on its way");
        assert_eq!(rows[1].1, RowAct::Var(vec!["Locals".into(), "o".into()], true));
    }

    #[test]
    fn what_comes_for_a_path_goes_under_it() {
        let mut scopes = vec![DebugScope { name: "Locals".into(), reference: 1, vars: Some(vec![var("o", "Order(1)", 7)]) }];
        let path = vec!["Locals".to_string(), "o".to_string()];
        assert_eq!(reference_at(&scopes, &path), Some((7, false)));
        *under_mut(&mut scopes, &path).unwrap() = Some(vec![var("qty", "3", 0)]);
        assert_eq!(reference_at(&scopes, &path), Some((7, true)));
        assert!(under_mut(&mut scopes, &["Locals".to_string(), "nope".to_string()]).is_none());
    }

    #[test]
    fn the_lens_reads_the_name_under_the_cursor_and_what_its_an_attribute_of() {
        let line = "    total = self.rows[0].qty * rate";
        assert_eq!(lens_expression(line, 6).as_deref(), Some("total"));
        assert_eq!(lens_expression(line, 17).as_deref(), Some("self.rows"), "on rows");
        assert_eq!(lens_expression(line, 13).as_deref(), Some("self"));
        assert_eq!(lens_expression(line, 26).as_deref(), Some("qty"), "after ], the name alone");
        assert_eq!(lens_expression(line, 10), None, "on =");
        assert_eq!(lens_expression("x = 42", 5), None, "a number");
    }

    #[test]
    fn the_lens_card_has_the_value_its_type_and_whats_in_it() {
        let card = lens_card("o", "Order(1042)", Some("Order"));
        assert_eq!(card, "o = Order(1042)\nOrder");
        let children: Vec<DebugVar> = (0..14).map(|i| var(&format!("f{i}"), "1", 0)).collect();
        let full = lens_card_with(&card, &children);
        assert!(full.starts_with("o = Order(1042)\nOrder\n  f0 = 1\n"), "{full}");
        assert!(full.ends_with("… 2 more in the drawer (SPC u f)"), "{full}");
    }

    #[test]
    fn the_shown_rows_follow_the_selection_and_no_further() {
        assert_eq!(keep_shown(0, 0, 30, 10), 0);
        assert_eq!(keep_shown(12, 0, 30, 10), 3);
        assert_eq!(keep_shown(5, 3, 30, 10), 3);
        assert_eq!(keep_shown(1, 3, 30, 10), 1);
        assert_eq!(keep_shown(2, 25, 5, 10), 0, "fewer rows than shown");
    }

    #[test]
    fn the_console_keeps_its_newest_lines() {
        let mut console = VecDeque::new();
        for i in 0..CONSOLE_LINES + 3 {
            console_push(&mut console, i.to_string());
        }
        assert_eq!((console.len(), console.front().map(String::as_str)), (CONSOLE_LINES, Some("3")));
    }
}
