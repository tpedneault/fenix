use crate::fuzzy::fuzzy_match;

/// One entry in a picker: `label` is what's displayed and fuzzy-matched
/// against; `payload` is whatever the caller actually wants back when the
/// entry is chosen (a `PathBuf` for find-file, a `GrepMatch` for search
/// results, etc.) -- kept separate so the picker never needs to know
/// anything about what it's picking between.
#[derive(Clone)]
pub struct Candidate<T> {
    pub label: String,
    pub payload: T,
}

impl<T> Candidate<T> {
    pub fn new(label: impl Into<String>, payload: T) -> Self {
        Self { label: label.into(), payload }
    }
}

struct Scored {
    index: usize,
    score: i64,
}

/// Live-filtered, fuzzy-ranked selection state: a query string, the full
/// candidate list, and which (ranked, filtered) row is currently
/// selected. Generic over the candidate payload so one implementation
/// serves find-file, grep results, and switch-project alike -- host- and
/// rendering-agnostic, the same role `fenix_keymap::KeyTrie` plays for
/// key sequences.
pub struct PickerState<T> {
    all: Vec<Candidate<T>>,
    query: String,
    filtered: Vec<Scored>,
    selected: usize,
}

impl<T> PickerState<T> {
    pub fn new(candidates: Vec<Candidate<T>>) -> Self {
        let mut state = Self { all: candidates, query: String::new(), filtered: Vec::new(), selected: 0 };
        state.refilter();
        state
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.refilter();
    }

    /// Replaces the whole query at once and refilters -- for callers that
    /// already know the exact new query string (e.g. a prefix re-derived
    /// fresh from a text buffer each keystroke) rather than editing it one
    /// character at a time via `push_char`/`backspace`.
    pub fn set_query(&mut self, query: &str) {
        self.query = query.to_string();
        self.refilter();
    }

    fn refilter(&mut self) {
        self.filtered = self
            .all
            .iter()
            .enumerate()
            .filter_map(|(index, c)| fuzzy_match(&self.query, &c.label).map(|score| Scored { index, score }))
            .collect();
        // Highest score first; stable tie-break by original order so
        // results don't jitter between keystrokes when scores are equal.
        self.filtered.sort_by(|a, b| b.score.cmp(&a.score).then(a.index.cmp(&b.index)));
        self.selected = 0;
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            return;
        }
        let len = self.filtered.len() as isize;
        let new = (self.selected as isize + delta).clamp(0, len - 1);
        self.selected = new as usize;
    }

    pub fn selected(&self) -> Option<&Candidate<T>> {
        self.filtered.get(self.selected).map(|s| &self.all[s.index])
    }

    pub fn selected_row(&self) -> usize {
        self.selected
    }

    pub fn len(&self) -> usize {
        self.filtered.len()
    }

    pub fn is_empty(&self) -> bool {
        self.filtered.is_empty()
    }

    /// The filtered/ranked candidates in `[offset, offset + count)`,
    /// paired with whether each one is the current selection -- for a
    /// caller windowing a possibly-long result list to what's actually on
    /// screen, the same shape `ExplorerState`'s rendering already windows
    /// a directory listing to.
    pub fn visible_rows(&self, offset: usize, count: usize) -> impl Iterator<Item = (bool, &Candidate<T>)> {
        let end = (offset + count).min(self.filtered.len());
        let start = offset.min(end);
        self.filtered[start..end].iter().enumerate().map(move |(i, s)| (start + i == self.selected, &self.all[s.index]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates(labels: &[&str]) -> Vec<Candidate<usize>> {
        labels.iter().enumerate().map(|(i, l)| Candidate::new(*l, i)).collect()
    }

    #[test]
    fn empty_query_shows_everything_in_original_order() {
        let picker = PickerState::new(candidates(&["b", "a", "c"]));
        let labels: Vec<&str> = picker.visible_rows(0, 10).map(|(_, c)| c.label.as_str()).collect();
        assert_eq!(labels, vec!["b", "a", "c"]);
    }

    #[test]
    fn push_char_filters_and_ranks() {
        let mut picker = PickerState::new(candidates(&["apple", "banana", "apricot"]));
        picker.push_char('a');
        picker.push_char('p');
        // "apple" and "apricot" both start with "ap" (contiguous, at
        // start); "banana" doesn't contain "ap" as a subsequence at all
        // in that order from position 0 the same way -- just check it's
        // excluded, and that both real matches are present.
        let labels: Vec<&str> = picker.visible_rows(0, 10).map(|(_, c)| c.label.as_str()).collect();
        assert!(labels.contains(&"apple"));
        assert!(labels.contains(&"apricot"));
        assert!(!labels.contains(&"banana"));
    }

    #[test]
    fn backspace_undoes_the_last_character_and_refilters() {
        let mut picker = PickerState::new(candidates(&["apple", "banana"]));
        picker.push_char('b');
        assert_eq!(picker.len(), 1);
        picker.backspace();
        assert_eq!(picker.len(), 2); // back to unfiltered
    }

    #[test]
    fn typing_resets_selection_to_the_top_match() {
        let mut picker = PickerState::new(candidates(&["a", "b", "c"]));
        picker.move_selection(2); // select "c"
        assert_eq!(picker.selected().unwrap().label, "c");
        picker.push_char('a'); // only "a" matches now
        assert_eq!(picker.selected_row(), 0);
        assert_eq!(picker.selected().unwrap().label, "a");
    }

    #[test]
    fn move_selection_clamps_at_both_ends() {
        let mut picker = PickerState::new(candidates(&["a", "b", "c"]));
        picker.move_selection(-5);
        assert_eq!(picker.selected_row(), 0);
        picker.move_selection(100);
        assert_eq!(picker.selected_row(), 2);
    }

    #[test]
    fn move_selection_on_empty_results_is_a_no_op() {
        let mut picker = PickerState::new(candidates(&["a"]));
        picker.push_char('z'); // no match
        assert!(picker.is_empty());
        picker.move_selection(1); // must not panic
        assert_eq!(picker.selected_row(), 0);
    }

    #[test]
    fn visible_rows_flags_the_selected_row_within_the_window() {
        let mut picker = PickerState::new(candidates(&["a", "b", "c", "d"]));
        picker.move_selection(2); // select "c", index 2
        let rows: Vec<(bool, &str)> = picker.visible_rows(1, 2).map(|(sel, c)| (sel, c.label.as_str())).collect();
        assert_eq!(rows, vec![(false, "b"), (true, "c")]);
    }

    #[test]
    fn payload_is_returned_alongside_the_label() {
        let picker = PickerState::new(candidates(&["only"]));
        assert_eq!(picker.selected().unwrap().payload, 0);
    }

    #[test]
    fn set_query_replaces_the_whole_query_and_refilters() {
        let mut picker = PickerState::new(candidates(&["apple", "banana", "apricot"]));
        picker.set_query("ap");
        let labels: Vec<&str> = picker.visible_rows(0, 10).map(|(_, c)| c.label.as_str()).collect();
        assert!(labels.contains(&"apple"));
        assert!(labels.contains(&"apricot"));
        assert!(!labels.contains(&"banana"));
        assert_eq!(picker.query(), "ap");

        picker.set_query("ban");
        let labels: Vec<&str> = picker.visible_rows(0, 10).map(|(_, c)| c.label.as_str()).collect();
        assert_eq!(labels, vec!["banana"]);
    }

    #[test]
    fn set_query_resets_selection_like_push_char_does() {
        let mut picker = PickerState::new(candidates(&["a", "b", "c"]));
        picker.move_selection(2);
        picker.set_query("");
        assert_eq!(picker.selected_row(), 0);
    }
}
