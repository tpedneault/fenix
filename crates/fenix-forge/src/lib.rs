//! What Fenix knows about a code-review forge, in terms that aren't
//! any one forge's.
//!
//! One trait plus a small neutral model, and no I/O of its own. The
//! only implementation today is `fenix-gitlab`, which is exactly why
//! this crate exists separately: the panel is written against these
//! types rather than against GitLab's JSON, so adding GitHub later is
//! a second implementation rather than a rewrite of the UI. Nothing
//! here mentions `iid`, `PRIVATE-TOKEN`, or `/api/v4`.
//!
//! The model is deliberately lossy. It carries what a review actually
//! needs -- who wrote it, what it changes, whether it can merge, what's
//! blocking it -- and drops everything a forge's API happens to return
//! alongside. A field earns its place here by being something the UI
//! shows or acts on.

/// Where a merge request is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MrState {
    Open,
    Merged,
    Closed,
    /// Open but locked against changes -- rare, and shown rather than
    /// silently folded into `Open`, since it explains why an action
    /// that should work doesn't.
    Locked,
}

impl MrState {
    pub fn label(self) -> &'static str {
        match self {
            MrState::Open => "open",
            MrState::Merged => "merged",
            MrState::Closed => "closed",
            MrState::Locked => "locked",
        }
    }
}

/// A CI run's outcome, normalized across forges.
///
/// `Other` keeps the forge's own word rather than mapping an unknown
/// state onto a known one: inventing "failed" for a status this doesn't
/// recognize would be worse than showing the raw string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineStatus {
    Success,
    Failed,
    Running,
    Pending,
    Canceled,
    Skipped,
    /// Waiting for someone to press a button.
    Manual,
    Other(String),
}

impl PipelineStatus {
    pub fn label(&self) -> &str {
        match self {
            PipelineStatus::Success => "passed",
            PipelineStatus::Failed => "failed",
            PipelineStatus::Running => "running",
            PipelineStatus::Pending => "pending",
            PipelineStatus::Canceled => "canceled",
            PipelineStatus::Skipped => "skipped",
            PipelineStatus::Manual => "manual",
            PipelineStatus::Other(raw) => raw,
        }
    }

    /// Whether this is a state worth stopping on -- what a UI colors as
    /// bad rather than merely as "not done yet".
    pub fn is_bad(&self) -> bool {
        matches!(self, PipelineStatus::Failed | PipelineStatus::Canceled)
    }
}

/// The three commit SHAs a line-anchored review comment has to quote to
/// say *which version* of a file it's about.
///
/// Carried on every merge request even though nothing in the current
/// milestone reads them: they're only obtainable from the same call
/// that fetches the merge request, and a comment posted against the
/// wrong version lands on the wrong line or is rejected outright.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffRefs {
    /// The target branch's tip when the diff was computed.
    pub base_sha: String,
    /// The source branch's tip.
    pub head_sha: String,
    /// The merge base the diff is measured from.
    pub start_sha: String,
}

/// A merge request, reduced to what a review needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRequest {
    /// The number people say out loud and type into a URL -- `!42`, not
    /// the forge's internal database id, which nothing here ever needs.
    pub number: u64,
    pub title: String,
    pub description: String,
    pub state: MrState,
    /// Explicitly marked not-ready-for-review by its author.
    pub draft: bool,
    pub source_branch: String,
    pub target_branch: String,
    pub author: String,
    pub web_url: String,
    /// The source branch can't be merged as-is.
    pub has_conflicts: bool,
    /// Tip of the source branch.
    pub sha: String,
    pub diff_refs: DiffRefs,
    pub comment_count: usize,
    pub pipeline: Option<PipelineStatus>,
    /// The forge's own timestamp string, shown as-is. Not parsed into a
    /// date type: it's displayed and sorted by the forge, never
    /// arithmetic, and every forge formats it differently.
    pub updated_at: String,
}

impl MergeRequest {
    /// `!42` -- how a merge request is referred to in conversation.
    pub fn reference(&self) -> String {
        format!("!{}", self.number)
    }

    /// The local branch name `checkout` creates for this merge request.
    ///
    /// Prefixed rather than reusing `source_branch`: the source branch
    /// may not exist locally, may exist and point somewhere else, and
    /// for a fork-sourced request may collide with an unrelated local
    /// branch of the same name. `mr-42` can only ever mean one thing.
    pub fn local_branch(&self) -> String {
        format!("mr-{}", self.number)
    }
}

/// How many approvals a merge request has and how many it still needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Approvals {
    pub approved: bool,
    pub required: usize,
    pub left: usize,
    pub approved_by: Vec<String>,
}

/// What happened to one file in a merge request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    Added,
    Modified,
    Deleted,
    Renamed,
}

impl FileChange {
    /// The single letter a file listing leads with, matching `git
    /// status`'s own vocabulary so one set of letters means one thing
    /// everywhere in Fenix.
    pub fn letter(self) -> char {
        match self {
            FileChange::Added => 'A',
            FileChange::Modified => 'M',
            FileChange::Deleted => 'D',
            FileChange::Renamed => 'R',
        }
    }
}

/// One changed file, with its diff as unified-diff text.
///
/// The diff arrives as text rather than as a parsed structure because
/// `fenix-diff` already parses exactly this, and re-encoding it into
/// some intermediate form here would mean two parsers to keep in step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub old_path: String,
    pub new_path: String,
    pub change: FileChange,
    /// The file's hunks, as they'd appear in `git diff` output -- but
    /// *without* the `diff --git`/`---`/`+++` preamble, which forges
    /// omit. `unified_diff` puts one back.
    pub diff: String,
}

impl ChangedFile {
    /// The path to show and act on: the new one, except for a deletion,
    /// where the old path is the only real name the file ever had.
    pub fn display_path(&self) -> &str {
        if self.change == FileChange::Deleted {
            &self.old_path
        } else {
            &self.new_path
        }
    }

    /// The file's diff with a `diff --git`/`---`/`+++` header in front,
    /// so `fenix_diff::parse` sees a well-formed file entry.
    ///
    /// Forges hand back hunks alone, with the paths in sibling JSON
    /// fields; the parser keys off `diff --git` to know a new file has
    /// started, so without a header every file's hunks would fold into
    /// whichever file came before.
    pub fn unified_diff(&self) -> String {
        let (old, new) = (&self.old_path, &self.new_path);
        let mut out = format!("diff --git a/{old} b/{new}\n");
        match self.change {
            FileChange::Added => out.push_str("new file mode 100644\n"),
            FileChange::Deleted => out.push_str("deleted file mode 100644\n"),
            FileChange::Renamed => out.push_str(&format!("rename from {old}\nrename to {new}\n")),
            FileChange::Modified => {}
        }
        // `/dev/null` on the side the file doesn't exist on, which is
        // what git itself writes and what the parser's own status
        // handling expects.
        let old_side = if self.change == FileChange::Added { "/dev/null".to_string() } else { format!("a/{old}") };
        let new_side = if self.change == FileChange::Deleted { "/dev/null".to_string() } else { format!("b/{new}") };
        out.push_str(&format!("--- {old_side}\n+++ {new_side}\n"));
        out.push_str(&self.diff);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out
    }
}

/// Where on a diff a review comment is anchored.
///
/// The three SHAs are not decoration: a forge stores a comment against
/// a specific *version* of the diff, and one quoted from a stale fetch
/// either lands on the wrong line or is rejected outright. They come
/// from the same call that fetched the merge request, which is why
/// `MergeRequest` carries them even though only this reads them.
///
/// Exactly one of `old_line`/`new_line` is set for an added or removed
/// line; a context line has both, and a forge may return either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    pub base_sha: String,
    pub head_sha: String,
    pub start_sha: String,
    pub old_path: String,
    pub new_path: String,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
}

impl Position {
    /// A comment on the *new* side of a file -- what commenting on an
    /// added or context line means.
    pub fn on_new_line(refs: &DiffRefs, old_path: &str, new_path: &str, line: usize) -> Self {
        Position {
            base_sha: refs.base_sha.clone(),
            head_sha: refs.head_sha.clone(),
            start_sha: refs.start_sha.clone(),
            old_path: old_path.to_string(),
            new_path: new_path.to_string(),
            old_line: None,
            new_line: Some(line),
        }
    }

    /// A comment on the *old* side -- the only side a removed line has.
    pub fn on_old_line(refs: &DiffRefs, old_path: &str, new_path: &str, line: usize) -> Self {
        Position {
            base_sha: refs.base_sha.clone(),
            head_sha: refs.head_sha.clone(),
            start_sha: refs.start_sha.clone(),
            old_path: old_path.to_string(),
            new_path: new_path.to_string(),
            old_line: Some(line),
            new_line: None,
        }
    }

    /// Whether this anchors to the same diff line as `(old, new)`.
    ///
    /// Lenient on purpose: a forge records a context line's position
    /// with both numbers, or with just the side that was clicked, so
    /// matching the pair exactly would silently drop threads. Whichever
    /// side the position names has to agree; a side it doesn't name
    /// isn't consulted.
    pub fn anchors_to(&self, old: Option<usize>, new: Option<usize>) -> bool {
        match (self.new_line, self.old_line) {
            (Some(n), _) if new == Some(n) => true,
            (Some(_), Some(o)) | (None, Some(o)) => old == Some(o),
            _ => false,
        }
    }
}

/// One comment in a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub created_at: String,
    /// Written by the forge itself ("changed the description",
    /// "approved this merge request"), not by a person. Kept rather
    /// than dropped at parse time so the panel can decide -- but never
    /// shown inline on a diff, where it is pure noise.
    pub system: bool,
}

/// A review thread: one comment and its replies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discussion {
    pub id: String,
    pub notes: Vec<Note>,
    pub resolved: bool,
    /// Whether resolving is even possible. A plain comment on the merge
    /// request (as opposed to on a diff line) usually is not, and
    /// offering the key anyway would just produce a forge error.
    pub resolvable: bool,
    /// Where on the diff this hangs, or `None` for a comment on the
    /// merge request as a whole.
    pub position: Option<Position>,
}

impl Discussion {
    /// The comment that started the thread -- what a one-line summary
    /// shows.
    pub fn first(&self) -> Option<&Note> {
        self.notes.first()
    }

    /// Whether this is a real review thread rather than the forge
    /// narrating its own events.
    pub fn is_human(&self) -> bool {
        self.notes.iter().any(|n| !n.system)
    }
}

/// What to do with the source branch and history when merging.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeOptions {
    pub squash: bool,
    pub remove_source_branch: bool,
    /// The head SHA the merge is expected to apply to. Sent so the
    /// forge refuses rather than merging something that moved under you
    /// between reading the diff and pressing the key.
    pub sha: Option<String>,
}

/// Which merge requests to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MrFilter {
    /// Opened by the authenticated user.
    Mine,
    /// Assigned to, or requested for review by, the authenticated user.
    ForMe,
    /// Every open one in the project.
    AllOpen,
}

impl MrFilter {
    pub fn label(self) -> &'static str {
        match self {
            MrFilter::Mine => "mine",
            MrFilter::ForMe => "for me",
            MrFilter::AllOpen => "all open",
        }
    }

    /// The next filter in the cycle one key steps through.
    pub fn next(self) -> Self {
        match self {
            MrFilter::Mine => MrFilter::ForMe,
            MrFilter::ForMe => MrFilter::AllOpen,
            MrFilter::AllOpen => MrFilter::Mine,
        }
    }
}

/// A code-review forge Fenix can read merge requests from.
///
/// Every method returns `Result<_, String>` rather than a typed error:
/// the only thing any caller does with a failure is show it, and a
/// forge's own message ("401 Unauthorized", "project not found") is far
/// more useful than a variant name Fenix invented. Same reasoning
/// `fenix-git`'s own action functions already use.
pub trait Forge {
    /// A short name for the project these merge requests belong to,
    /// for a pane heading -- `group/project`.
    fn project(&self) -> &str;

    fn list_merge_requests(&self, filter: MrFilter) -> Result<Vec<MergeRequest>, String>;

    fn merge_request(&self, number: u64) -> Result<MergeRequest, String>;

    fn approvals(&self, number: u64) -> Result<Approvals, String>;

    fn changed_files(&self, number: u64) -> Result<Vec<ChangedFile>, String>;

    /// The refspec that fetches this merge request's head into a local
    /// branch, for `git fetch <remote> <refspec>`.
    ///
    /// Forge-specific by nature -- GitLab publishes
    /// `refs/merge-requests/N/head`, GitHub `refs/pull/N/head` -- which
    /// is exactly why it's on the trait rather than assembled by the
    /// caller.
    fn checkout_refspec(&self, number: u64) -> String;

    // -- Review ---------------------------------------------------------
    //
    // Everything below writes to the forge. Each is one deliberate
    // keypress in a view built for exactly that, and each returns the
    // forge's own message on failure -- a merge refused for an
    // unresolved thread says so in words no error enum here could
    // improve on.

    fn discussions(&self, number: u64) -> Result<Vec<Discussion>, String>;

    /// Adds a note to an existing thread.
    fn reply(&self, number: u64, discussion: &str, body: &str) -> Result<(), String>;

    /// Marks a thread resolved, or reopens it.
    fn resolve(&self, number: u64, discussion: &str, resolved: bool) -> Result<(), String>;

    /// Starts a new thread anchored to a diff line.
    fn comment_on_line(&self, number: u64, position: &Position, body: &str) -> Result<(), String>;

    /// Approves, or withdraws an approval. `sha` is the head the
    /// approval is for, so approving a version you have not seen is
    /// refused rather than silently recorded.
    fn approve(&self, number: u64, sha: Option<&str>) -> Result<(), String>;

    fn unapprove(&self, number: u64) -> Result<(), String>;

    fn merge(&self, number: u64, options: &MergeOptions) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs() -> DiffRefs {
        DiffRefs { base_sha: "b".to_string(), head_sha: "h".to_string(), start_sha: "s".to_string() }
    }

    #[test]
    fn a_new_side_comment_names_only_the_new_line() {
        let p = Position::on_new_line(&refs(), "a.rs", "a.rs", 12);
        assert_eq!(p.new_line, Some(12));
        assert_eq!(p.old_line, None);
        // The SHAs come along, because a comment stored against the
        // wrong version lands on the wrong line.
        assert_eq!((p.base_sha.as_str(), p.head_sha.as_str(), p.start_sha.as_str()), ("b", "h", "s"));
    }

    #[test]
    fn an_old_side_comment_names_only_the_old_line() {
        let p = Position::on_old_line(&refs(), "a.rs", "a.rs", 4);
        assert_eq!(p.old_line, Some(4));
        assert_eq!(p.new_line, None);
    }

    #[test]
    fn a_position_matches_the_diff_line_it_names() {
        let added = Position::on_new_line(&refs(), "a.rs", "a.rs", 12);
        assert!(added.anchors_to(None, Some(12)));
        assert!(!added.anchors_to(None, Some(13)));

        let removed = Position::on_old_line(&refs(), "a.rs", "a.rs", 4);
        assert!(removed.anchors_to(Some(4), None));
        assert!(!removed.anchors_to(Some(5), None));
    }

    #[test]
    fn a_context_line_matches_whichever_side_the_forge_recorded() {
        // A forge records a context line's position with both numbers,
        // or just the side that was clicked -- matching the pair
        // exactly would silently drop those threads.
        let mut both = Position::on_new_line(&refs(), "a.rs", "a.rs", 12);
        both.old_line = Some(9);
        assert!(both.anchors_to(Some(9), Some(12)));
        assert!(both.anchors_to(Some(9), None), "the old side alone still matches");
        assert!(both.anchors_to(None, Some(12)), "and so does the new side alone");
    }

    #[test]
    fn a_thread_of_only_forge_narration_is_not_a_human_one() {
        let note = |system: bool| Note { id: 1, author: "a".to_string(), body: "b".to_string(), created_at: String::new(), system };
        let narration =
            Discussion { id: "d".to_string(), notes: vec![note(true)], resolved: false, resolvable: false, position: None };
        assert!(!narration.is_human());
        let real = Discussion { id: "d".to_string(), notes: vec![note(false)], resolved: false, resolvable: true, position: None };
        assert!(real.is_human());
        assert_eq!(real.first().map(|n| n.id), Some(1));
    }

    fn file(change: FileChange, old: &str, new: &str) -> ChangedFile {
        ChangedFile { old_path: old.to_string(), new_path: new.to_string(), change, diff: "@@ -1 +1 @@\n-a\n+b\n".to_string() }
    }

    #[test]
    fn a_modified_files_diff_gets_an_ordinary_git_header() {
        let text = file(FileChange::Modified, "src/a.rs", "src/a.rs").unified_diff();
        assert!(text.starts_with("diff --git a/src/a.rs b/src/a.rs\n"));
        assert!(text.contains("--- a/src/a.rs\n+++ b/src/a.rs\n"));
        assert!(text.ends_with("@@ -1 +1 @@\n-a\n+b\n"));
    }

    #[test]
    fn an_added_file_gets_dev_null_on_the_old_side() {
        let text = file(FileChange::Added, "src/new.rs", "src/new.rs").unified_diff();
        assert!(text.contains("new file mode 100644\n"));
        assert!(text.contains("--- /dev/null\n+++ b/src/new.rs\n"), "got:\n{text}");
    }

    #[test]
    fn a_deleted_file_gets_dev_null_on_the_new_side() {
        let text = file(FileChange::Deleted, "src/old.rs", "src/old.rs").unified_diff();
        assert!(text.contains("deleted file mode 100644\n"));
        assert!(text.contains("--- a/src/old.rs\n+++ /dev/null\n"), "got:\n{text}");
    }

    #[test]
    fn a_rename_names_both_paths_in_the_header() {
        let text = file(FileChange::Renamed, "old.rs", "new.rs").unified_diff();
        assert!(text.contains("rename from old.rs\nrename to new.rs\n"), "got:\n{text}");
    }

    #[test]
    fn a_diff_missing_its_trailing_newline_gets_one() {
        let mut f = file(FileChange::Modified, "a", "a");
        f.diff = "@@ -1 +1 @@\n-a\n+b".to_string();
        assert!(f.unified_diff().ends_with("+b\n"));
    }

    #[test]
    fn the_display_path_is_the_only_real_name_a_deleted_file_had() {
        assert_eq!(file(FileChange::Deleted, "gone.rs", "gone.rs").display_path(), "gone.rs");
        assert_eq!(file(FileChange::Renamed, "old.rs", "new.rs").display_path(), "new.rs");
    }

    #[test]
    fn a_merge_requests_local_branch_is_named_after_its_number() {
        let mr = MergeRequest {
            number: 42,
            title: String::new(),
            description: String::new(),
            state: MrState::Open,
            draft: false,
            source_branch: "feature".to_string(),
            target_branch: "main".to_string(),
            author: String::new(),
            web_url: String::new(),
            has_conflicts: false,
            sha: String::new(),
            diff_refs: DiffRefs::default(),
            comment_count: 0,
            pipeline: None,
            updated_at: String::new(),
        };
        // Not `source_branch`: it may not exist locally, may point
        // somewhere else, or may collide with an unrelated branch.
        assert_eq!(mr.local_branch(), "mr-42");
        assert_eq!(mr.reference(), "!42");
    }

    #[test]
    fn an_unrecognized_pipeline_status_keeps_the_forges_own_word() {
        let status = PipelineStatus::Other("waiting_for_resource".to_string());
        assert_eq!(status.label(), "waiting_for_resource");
        assert!(!status.is_bad(), "an unknown state isn't assumed to be a failure");
    }

    #[test]
    fn the_filter_cycles_through_all_three() {
        let mut f = MrFilter::Mine;
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(f.label());
            f = f.next();
        }
        assert_eq!(seen, vec!["mine", "for me", "all open"]);
        assert_eq!(f, MrFilter::Mine, "and back to the start");
    }
}
