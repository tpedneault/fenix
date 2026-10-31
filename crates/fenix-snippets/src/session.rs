use crate::{Context, Rendered, Template};
use fenix_core::{Buffer, Cursor};
use std::{collections::BTreeMap, ops::Range};

pub enum Edit<'a> {
    Insert(&'a str),
    Backspace,
    Delete,
}

/// One active expansion. Unexpected edits or cursor movement invalidate it,
/// so undo, reload, mouse movement and external edits cannot use stale ranges.
pub struct Session {
    template: Template,
    context: Context,
    indent: String,
    values: BTreeMap<u32, String>,
    rendered: Rendered,
    start: usize,
    stops: Vec<u32>,
    active: usize,
    selected: bool,
    revision: u64,
    cursor: usize,
}

impl Session {
    pub fn expand(
        buffer: &mut Buffer,
        cursor: &mut Cursor,
        trigger_start: usize,
        template: Template,
        context: Context,
    ) -> Option<Self> {
        let line = buffer.line_col(cursor).0;
        let indent: String = buffer
            .text_range(buffer.line_start_char(line), trigger_start)
            .chars()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .collect();
        let values = template.defaults.clone();
        let rendered = template.render(&values, &context, &indent);
        let stops = rendered.fields.keys().copied().collect::<Vec<_>>();
        let trigger_end = cursor.char_idx;
        buffer.replace_range(cursor, trigger_start, trigger_end, &rendered.text);
        if stops.is_empty() {
            cursor.char_idx = trigger_start + rendered.exit;
            cursor.sticky_col = buffer.line_col(cursor).1;
            return None;
        }
        let mut session = Self {
            template,
            context,
            indent,
            values,
            rendered,
            start: trigger_start,
            stops,
            active: 0,
            selected: true,
            revision: buffer.edit_count(),
            cursor: 0,
        };
        session.select(cursor);
        cursor.sticky_col = buffer.line_col(cursor).1;
        Some(session)
    }

    fn range(&self) -> Range<usize> {
        let range = &self.rendered.fields[&self.stops[self.active]];
        self.start + range.start..self.start + range.end
    }

    fn select(&mut self, cursor: &mut Cursor) {
        self.selected = true;
        cursor.char_idx = self.range().start;
        cursor.sticky_col = 0;
        self.cursor = cursor.char_idx;
    }

    pub fn valid(&self, buffer: &Buffer, cursor: &Cursor) -> bool {
        self.revision == buffer.edit_count() && self.cursor == cursor.char_idx
    }

    pub fn selection(&self) -> Option<Range<usize>> {
        self.selected.then(|| self.range())
    }

    /// False means the final stop was reached and the caller must drop session.
    pub fn advance(&mut self, cursor: &mut Cursor, backwards: bool) -> bool {
        if backwards {
            self.active = self.active.saturating_sub(1);
        } else {
            self.active += 1;
            if self.active == self.stops.len() {
                cursor.char_idx = self.start + self.rendered.exit;
                cursor.sticky_col = 0;
                return false;
            }
        }
        self.select(cursor);
        true
    }

    pub fn edit(&mut self, buffer: &mut Buffer, cursor: &mut Cursor, edit: Edit<'_>) {
        let id = self.stops[self.active];
        let mut value = self.values[&id].clone();
        // While typing the caret is always at the end; arrows leave snippet
        // mode. Revisiting a field selects the whole value for replacement.
        if self.selected {
            value.clear();
        }
        match edit {
            Edit::Insert(text) => value.push_str(text),
            Edit::Backspace if !self.selected => {
                value.pop();
            }
            Edit::Backspace | Edit::Delete => {}
        }
        self.values.insert(id, value);
        let next = self
            .template
            .render(&self.values, &self.context, &self.indent);
        if next.text != self.rendered.text {
            buffer.replace_range(
                cursor,
                self.start,
                self.start + self.rendered.text.chars().count(),
                &next.text,
            );
        }
        self.rendered = next;
        self.selected = false;
        cursor.char_idx = self.range().end;
        cursor.sticky_col = buffer.line_col(cursor).1;
        self.cursor = cursor.char_idx;
        self.revision = buffer.edit_count();
    }
}
