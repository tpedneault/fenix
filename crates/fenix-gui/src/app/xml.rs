//! XML editing on top of the grammar: closing tags as they're opened,
//! `%` between a start tag and its end tag, an element outline, a well-
//! formedness check, and copying an element's XPath. Everything here is
//! a no-op outside an XML buffer.

use super::*;
use fenix_syntax::LanguageId;

impl App {
    fn focused_is_xml(&self) -> bool {
        self.open().kind == BufferKind::Text && self.focused_language() == Some(LanguageId::Xml)
    }

    /// Runs right after a key reached Vim in Insert mode, with `typed`
    /// the character it inserted. Typing the `>` that finishes a start
    /// tag inserts its end tag after the cursor (`<item>|</item>`);
    /// typing the `/` of `</` completes the innermost element still open
    /// (`</|` -> `</item>|`). Neither happens outside XML, when the
    /// character wasn't actually inserted, or when the end tag is
    /// already sitting right there.
    pub(super) fn xml_autoclose(&mut self, typed: char) {
        if !self.focused_is_xml() {
            return;
        }
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        let at = cursor.char_idx;
        if at == 0 || buffer.char_at(at - 1) != Some(typed) {
            return;
        }
        let before = buffer.text_range(0, at);
        let after_len = buffer.len_chars().saturating_sub(at).min(256);
        let after = buffer.text_range(at, at + after_len);
        match typed {
            '>' => {
                let Some(close) = fenix_syntax::xml::close_tag_after_gt(&before) else { return };
                if after.starts_with(&close) {
                    return;
                }
                buffer.insert_str(cursor, &close);
                cursor.char_idx = at;
            }
            '/' => {
                let Some(opener) = before.strip_suffix("</") else { return };
                let Some(name) = fenix_syntax::xml::innermost_open_element(opener) else { return };
                // Someone typing the name themselves after `</` would
                // otherwise get it twice.
                if after.starts_with(|c: char| c.is_alphanumeric() || c == '>') {
                    return;
                }
                buffer.insert_str(cursor, &format!("{name}>"));
            }
            _ => return,
        }
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
        if !self.replaying_change {
            self.change_capture_dirty = true;
        }
    }

    /// `%` in an XML buffer with the cursor on a start or end tag: jumps
    /// to the other one. Returns whether it did -- anywhere else `%`
    /// falls through to Vim's own bracket matching untouched, so it
    /// still works on the brackets inside an attribute or a DTD.
    pub(super) fn xml_jump_matching_tag(&mut self) -> bool {
        if !self.focused_is_xml() {
            return false;
        }
        let id = self.focused_buffer_id();
        self.syntax_highlights_for_visible_range(id, 0, 1);
        let ob = self.open();
        let Some(syntax) = &ob.syntax else { return false };
        let source = ob.buffer.text();
        let cursor_idx = self.cursor().char_idx;
        let Some(target) = syntax.xml_matching_tag(&source, ob.buffer.char_to_byte(cursor_idx)) else { return false };
        let target = ob.buffer.byte_to_char(target);
        let from = JumpEntry { buffer: id, char_idx: cursor_idx };
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        cursor.char_idx = target;
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
        self.record_jump(from);
        true
    }

    /// `SPC c o` in an XML buffer: every element as an indented tree,
    /// named by its tag and, when it has one, an identifying attribute
    /// (`dependency  id=core`), so a long run of same-named siblings can
    /// still be told apart and filtered by what makes them different.
    pub(super) fn picker_xml_outline(&mut self) {
        let id = self.focused_buffer_id();
        self.syntax_highlights_for_visible_range(id, 0, 1);
        let ob = self.open();
        let Some(syntax) = &ob.syntax else { return };
        let source = ob.buffer.text();
        let candidates: Vec<_> = syntax
            .xml_elements(&source)
            .into_iter()
            .map(|element| {
                let attribute = element.label_attribute.map(|(name, value)| format!("  {name}={value}")).unwrap_or_default();
                let label = format!("{}{}{attribute}  ·  line {}", "  ".repeat(element.depth), element.name, element.start_row + 1);
                fenix_picker::Candidate::new(label, ob.buffer.byte_to_char(element.start_byte))
            })
            .collect();
        if candidates.is_empty() {
            self.set_message("no elements in this document");
            return;
        }
        self.enter_picker(ActivePicker::Outline(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC c v`: checks the focused XML buffer is well-formed. On an
    /// error, moves the cursor to where the parser gave up and says
    /// what it found there.
    pub(crate) fn xml_validate(&mut self) {
        if !self.focused_is_xml() {
            self.set_error("SPC c v needs an XML buffer");
            return;
        }
        match fenix_syntax::xml::check_well_formed(&self.open().buffer.text()) {
            Ok(()) => self.set_message("well-formed XML"),
            Err(err) => {
                let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
                let line = err.line.min(buffer.line_count().saturating_sub(1));
                let col = err.col.min(buffer.line_len(line));
                cursor.char_idx = buffer.line_start_char(line) + col;
                cursor.sticky_col = col;
                self.set_error(format!("not well-formed: {}", err.message));
            }
        }
        self.wake_caret();
    }

    /// After saving `path`: a note about it not being well-formed XML,
    /// if it's XML and isn't. The save itself has already happened --
    /// a half-edited document is still worth keeping -- this is only so
    /// a broken file isn't written without anyone noticing.
    pub(super) fn xml_save_problem(&self, path: &Path) -> Option<String> {
        if fenix_syntax::detect_language_from_path(path) != Some(LanguageId::Xml) {
            return None;
        }
        fenix_syntax::xml::check_well_formed(&self.open().buffer.text())
            .err()
            .map(|err| format!("not well-formed XML at {}:{}: {} (SPC c v to jump there)", err.line + 1, err.col + 1, err.message))
    }

    /// `SPC c y`: copies an XPath to the element under the cursor
    /// (`/project/dependencies/dependency[3]/version`) and shows it.
    pub(crate) fn yank_xml_path(&mut self) {
        if !self.focused_is_xml() {
            self.set_error("SPC c y needs an XML buffer");
            return;
        }
        let id = self.focused_buffer_id();
        self.syntax_highlights_for_visible_range(id, 0, 1);
        let ob = self.open();
        let Some(syntax) = &ob.syntax else { return };
        let byte = ob.buffer.char_to_byte(self.cursor().char_idx.min(ob.buffer.len_chars()));
        let Some(path) = syntax.xml_element_path(&ob.buffer.text(), byte) else {
            self.set_message("the cursor isn't inside an element");
            return;
        };
        if let Some(clipboard) = &mut self.clipboard {
            let _ = clipboard.set_text(path.clone());
        }
        self.set_message(format!("copied {path}"));
    }
}
