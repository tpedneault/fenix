//! A unified-diff parser and single-hunk patch synthesizer -- the shared
//! vocabulary behind every diff Fenix shows or acts on: the Git panel's
//! working-tree view, ref-to-ref comparison, conflict resolution, and
//! (later) GitLab merge-request review, where a review comment has to be
//! anchored to a specific old/new line number in a specific file.
//!
//! Deliberately pure: no I/O, no `git` invocation, no rendering. It
//! takes the text `git diff`/`git show` (or a forge API) already
//! produced and gives back a structure, plus the inverse operation --
//! turning one hunk back into a patch `git apply` accepts, which is how
//! per-hunk staging works here (the same mechanism lazygit and magit
//! use, rather than trying to rewrite the index directly).
//!
//! **Byte-exactness is the whole game.** A patch that differs from what
//! `git` expects by a single character doesn't fail loudly -- `git
//! apply` rejects it, or worse, applies something subtly wrong. So
//! every line's content is preserved verbatim, including a trailing
//! `\r` on a CRLF file (which is why this parser splits on `'\n'` and
//! never uses `str::lines`, whose documented behavior is to strip a
//! trailing `\r` -- exactly the corruption that would make hunk staging
//! silently mangle every CRLF file in a repo).

mod parse;
mod patch;

pub use parse::parse;
pub use patch::hunk_patch;

/// What happened to a file in this diff, as its own `diff --git` header
/// declares it -- `Modified` is the default because that's the case git
/// says nothing extra about (there's no `modified file mode` line; the
/// absence of any of the others is the signal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
}

/// One line inside a hunk. `kind` is the leading marker column, already
/// interpreted; `text` is everything *after* that one marker character,
/// verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
    /// `\ No newline at end of file` -- not a line of the file at all,
    /// but part of the patch text and load-bearing: dropping it when
    /// re-synthesizing a patch changes whether the result ends in a
    /// newline, which `git apply` treats as a real content difference.
    NoNewline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// This line's 1-based number in the *old* file, or `None` for an
    /// added line (it doesn't exist there) and for `NoNewline`.
    pub old_line: Option<usize>,
    /// This line's 1-based number in the *new* file, or `None` for a
    /// removed line and for `NoNewline`.
    pub new_line: Option<usize>,
    /// The line's content with its marker character removed, verbatim
    /// otherwise -- a trailing `\r` on a CRLF file is content and stays.
    pub text: String,
}

impl DiffLine {
    /// The line as it appears in patch text: its marker character
    /// followed by `text`. The exact inverse of what the parser strips,
    /// so `raw` -> parse -> `raw` is a fixed point.
    pub fn raw(&self) -> String {
        let marker = match self.kind {
            LineKind::Context => ' ',
            LineKind::Added => '+',
            LineKind::Removed => '-',
            LineKind::NoNewline => '\\',
        };
        format!("{marker}{}", self.text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: usize,
    pub old_len: usize,
    pub new_start: usize,
    pub new_len: usize,
    /// The `@@ -a,b +c,d @@` line verbatim, including any trailing
    /// section heading git appends (`@@ ... @@ fn foo() {`) -- kept whole
    /// rather than rebuilt from the four numbers, both because the
    /// heading is genuinely useful to show and because re-emitting the
    /// original bytes is one less way to produce a patch git dislikes.
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// Path as the diff names it, with git's `a/` prefix already
    /// stripped; `/dev/null` for an added file.
    pub old_path: String,
    /// Same, with the `b/` prefix stripped; `/dev/null` for a deleted
    /// file.
    pub new_path: String,
    pub status: FileStatus,
    /// A binary file git refused to show a textual diff for -- there are
    /// no hunks to render or stage, so callers show a placeholder
    /// instead (the same posture the Git panel already takes for an
    /// untracked file, which has no diff either).
    pub is_binary: bool,
    /// Every line from `diff --git ...` up to (not including) the first
    /// `@@`, verbatim -- the `index`/`new file mode`/`rename from`/
    /// `---`/`+++` preamble. Kept whole so `hunk_patch` can re-emit
    /// git's own header rather than reconstructing one and hoping it
    /// matches.
    pub header: Vec<String>,
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    /// The path to show and to act on: the new path, except for a
    /// deletion, where the new path is `/dev/null` and the old one is
    /// the only real name the file ever had.
    pub fn display_path(&self) -> &str {
        if self.status == FileStatus::Deleted {
            &self.old_path
        } else {
            &self.new_path
        }
    }
}
