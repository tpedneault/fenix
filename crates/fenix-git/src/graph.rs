//! The commit graph: `git log --all`'s topology, plus the lane
//! assignment that turns a list of commits-and-their-parents into
//! something drawable as rails.
//!
//! Split deliberately: this module knows about parents and lanes but
//! nothing about glyphs or colors, and `fenix-gui`'s `graph_view` knows
//! about glyphs but never has to re-derive the topology. `assign_lanes`
//! is pure -- it takes commits and returns rows -- so the interesting
//! part (what happens at a merge, at a branch tip, at two independent
//! roots) is testable against hand-built histories rather than only
//! against whatever a real repo happens to contain.

use std::path::Path;

use crate::process::run_lines;

/// One commit as the graph needs it: its own identity, its parents (the
/// edges), and whatever refs point at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphCommit {
    pub hash: String,
    pub short_hash: String,
    /// Parent hashes, first-parent first -- the order git reports, which
    /// is what makes "the first parent continues this branch's lane"
    /// meaningful for a merge.
    pub parents: Vec<String>,
    /// Ref decorations from `%D`, already split: `HEAD -> main`,
    /// `origin/main`, `tag: v1.0`. Kept as git's own strings rather than
    /// re-parsed into kinds -- the renderer wants to show them, and
    /// `refs/` prefixes are already stripped by `%D`.
    pub refs: Vec<String>,
    pub author: String,
    pub relative_date: String,
    pub subject: String,
}

/// Where one commit sits in the drawn graph, and which lanes are live
/// around it.
///
/// `live_before`/`live_after` are given rather than a single "rails on
/// this row" list because a renderer that draws connector rows (the
/// `|\` under a merge, the `|/` above a shared ancestor -- the same
/// shape `git log --graph` uses) needs both sides: the connector above a
/// commit is drawn against the lanes that existed before it, the one
/// below against the lanes that exist after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    /// The lane this commit's node sits in.
    pub lane: usize,
    /// Lanes occupied immediately before this commit -- including any
    /// converging here (`closing`), and excluding this commit's own lane
    /// when it's a branch tip nothing was heading toward yet.
    pub live_before: Vec<usize>,
    /// Lanes occupied immediately after -- including `lane` (unless this
    /// is a root commit, whose lane goes free) and any this commit
    /// opened.
    pub live_after: Vec<usize>,
    /// Lanes that ended at this commit: others that were also waiting
    /// for this exact hash, i.e. branches converging here.
    pub closing: Vec<usize>,
    /// Lanes carrying this commit's second and later parents -- the
    /// visual "this merge brought in another line of history".
    pub branching: Vec<usize>,
    pub is_merge: bool,
}

/// Every commit reachable from any ref (`--all`), newest first by date --
/// the order a graph is read in, and the one that keeps a merge's two
/// sides interleaved by time rather than one branch being drawn entirely
/// before the other (`--topo-order`'s behavior, which reads as
/// misleadingly linear).
///
/// Not `--date-order`: that promises no parent before its children, and
/// to keep the promise git walks the *whole* history before printing the
/// first line -- seconds on a large repository, for the newest 200
/// commits. Git's plain order streams; `children_first` then keeps the
/// promise for the commits listed, which is all the graph needs.
pub fn commit_graph(repo: &Path, limit: usize) -> Vec<GraphCommit> {
    let n = format!("-n{limit}");
    let lines = run_lines(repo, &["log", "--all", &n, "--format=%H\x1f%h\x1f%P\x1f%D\x1f%an\x1f%ar\x1f%s"]);
    children_first(lines.iter().filter_map(|l| parse_line(l)).collect())
}

/// `commits` with every one listed after all of its children that are
/// listed, otherwise in the order given -- what `assign_lanes` needs. Git
/// lists by commit date, and a clock that was wrong when a commit was
/// made can date it before its parent.
pub fn children_first(commits: Vec<GraphCommit>) -> Vec<GraphCommit> {
    use std::cmp::Reverse;
    use std::collections::{BinaryHeap, HashMap};

    let at: HashMap<&str, usize> = commits.iter().enumerate().map(|(i, c)| (c.hash.as_str(), i)).collect();
    // How many listed children each commit waits for.
    let mut waiting = vec![0usize; commits.len()];
    for c in &commits {
        for p in &c.parents {
            if let Some(&i) = at.get(p.as_str()) {
                waiting[i] += 1;
            }
        }
    }
    // Almost always already so: then there's nothing to do.
    let in_order = commits.iter().enumerate().all(|(i, c)| c.parents.iter().all(|p| at.get(p.as_str()).is_none_or(|&j| j > i)));
    if in_order {
        return commits;
    }
    let mut ready: BinaryHeap<Reverse<usize>> = (0..commits.len()).filter(|&i| waiting[i] == 0).map(Reverse).collect();
    let mut order = Vec::with_capacity(commits.len());
    while let Some(Reverse(i)) = ready.pop() {
        order.push(i);
        for p in &commits[i].parents {
            if let Some(&j) = at.get(p.as_str()) {
                waiting[j] -= 1;
                if waiting[j] == 0 {
                    ready.push(Reverse(j));
                }
            }
        }
    }
    drop(at);
    let mut slots: Vec<Option<GraphCommit>> = commits.into_iter().map(Some).collect();
    order.into_iter().filter_map(|i| slots[i].take()).collect()
}

fn parse_line(line: &str) -> Option<GraphCommit> {
    let mut fields = line.split('\x1f');
    let hash = fields.next()?.to_string();
    let short_hash = fields.next()?.to_string();
    let parents_raw = fields.next()?;
    let refs_raw = fields.next()?;
    let author = fields.next()?.to_string();
    let relative_date = fields.next()?.to_string();
    let subject = fields.next().unwrap_or("").to_string();
    Some(GraphCommit {
        hash,
        short_hash,
        // A root commit has no parents at all, and `%P` is then empty --
        // `split_whitespace` yields nothing, which is right.
        parents: parents_raw.split_whitespace().map(str::to_string).collect(),
        refs: refs_raw.split(", ").filter(|r| !r.is_empty()).map(str::to_string).collect(),
        author,
        relative_date,
        subject,
    })
}

/// Assigns each commit a lane, tracking which lanes are live across
/// rows.
///
/// The model is a set of lanes, each "waiting for" a specific commit
/// hash -- the hash whose row that lane's rail is heading toward. For
/// each commit, in the order given:
///
/// - Any lane waiting for this commit converges here. The first such
///   lane becomes the commit's own lane (so a branch keeps its column
///   as you scroll); the rest are `closing`.
/// - The commit's first parent inherits that lane, so a linear history
///   stays in one column forever.
/// - Each additional parent (a merge) claims a lane of its own --
///   reusing one already waiting for that parent if there is one, which
///   is what makes two branches that share an ancestor visibly rejoin
///   rather than drawing a redundant rail.
/// - A commit nothing is waiting for is a tip: it takes the leftmost
///   free lane.
///
/// Lanes are reused as soon as they go idle, so the graph stays as
/// narrow as the history actually is rather than growing a column per
/// branch ever seen.
pub fn assign_lanes(commits: &[GraphCommit]) -> Vec<GraphRow> {
    // `lanes[i]` is the hash lane `i` is currently heading toward, or
    // `None` if the lane is free.
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());

    for commit in commits {
        let live_before = occupied(&lanes);
        let waiting: Vec<usize> =
            lanes.iter().enumerate().filter(|(_, l)| l.as_deref() == Some(commit.hash.as_str())).map(|(i, _)| i).collect();

        let lane = match waiting.first() {
            Some(&i) => i,
            None => claim_free_lane(&mut lanes),
        };
        let closing: Vec<usize> = waiting.iter().skip(1).copied().collect();
        for &c in &closing {
            lanes[c] = None;
        }

        // The first parent continues in this commit's own lane; with no
        // parents at all (a root) the lane goes free.
        lanes[lane] = commit.parents.first().cloned();

        let mut branching = Vec::new();
        for parent in commit.parents.iter().skip(1) {
            match lanes.iter().position(|l| l.as_deref() == Some(parent.as_str())) {
                // Something already heads toward this parent -- draw the
                // merge edge into that existing lane instead of opening
                // a duplicate one beside it.
                Some(existing) => branching.push(existing),
                None => {
                    let i = claim_free_lane(&mut lanes);
                    lanes[i] = Some(parent.clone());
                    branching.push(i);
                }
            }
        }

        rows.push(GraphRow {
            lane,
            live_before,
            live_after: occupied(&lanes),
            closing,
            branching,
            is_merge: commit.parents.len() > 1,
        });
    }
    rows
}

/// The indices of every lane currently heading toward something.
fn occupied(lanes: &[Option<String>]) -> Vec<usize> {
    lanes.iter().enumerate().filter(|(_, l)| l.is_some()).map(|(i, _)| i).collect()
}

/// The leftmost free lane, growing the set only when every existing lane
/// is busy -- keeps the graph narrow and keeps a branch's column stable
/// for as long as it's alive.
fn claim_free_lane(lanes: &mut Vec<Option<String>>) -> usize {
    match lanes.iter().position(|l| l.is_none()) {
        Some(i) => i,
        None => {
            lanes.push(None);
            lanes.len() - 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{git, init_repo, TempDir};

    /// A commit with hash `h` and the given parents -- everything else
    /// is filler, since `assign_lanes` only ever reads hash/parents.
    fn c(h: &str, parents: &[&str]) -> GraphCommit {
        GraphCommit {
            hash: h.to_string(),
            short_hash: h.to_string(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            refs: Vec::new(),
            author: "Test".to_string(),
            relative_date: "now".to_string(),
            subject: h.to_string(),
        }
    }

    #[test]
    fn a_linear_history_stays_in_one_lane() {
        let rows = assign_lanes(&[c("c", &["b"]), c("b", &["a"]), c("a", &[])]);
        assert!(rows.iter().all(|r| r.lane == 0), "{rows:?}");
        assert!(rows.iter().all(|r| r.branching.is_empty() && r.closing.is_empty()));
        assert!(rows.iter().all(|r| !r.is_merge));
        // Only lane 0 is ever live, and the root frees it again.
        assert_eq!(rows[1].live_before, vec![0]);
        assert!(rows[2].live_after.is_empty(), "a root commit's lane goes free: {:?}", rows[2]);
    }

    #[test]
    fn a_second_tip_takes_its_own_lane_and_draws_a_rail_past_the_first() {
        // Two independent branch tips, as `--all` reports them.
        let rows = assign_lanes(&[c("x", &["a"]), c("y", &["a"]), c("a", &[])]);
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[1].lane, 1);
        // While `y` is drawn, lane 0 (heading toward `a`) is still live
        // and gets a rail beside it.
        assert_eq!(rows[1].live_before, vec![0]);
    }

    #[test]
    fn two_branches_converging_on_a_shared_ancestor_close_into_one_lane() {
        let rows = assign_lanes(&[c("x", &["a"]), c("y", &["a"]), c("a", &[])]);
        // `a` is reached by both lanes: it keeps the first and closes
        // the second, rather than being drawn twice.
        assert_eq!(rows[2].lane, 0);
        assert_eq!(rows[2].closing, vec![1]);
        // Both lanes were live coming in; only lane 0 survives.
        assert_eq!(rows[2].live_before, vec![0, 1]);
        assert!(rows[2].live_after.is_empty(), "`a` is a root, so nothing is left heading anywhere");
    }

    #[test]
    fn a_merge_opens_a_lane_for_its_second_parent() {
        // m merges side into main: parents are [main-tip, side-tip].
        let rows = assign_lanes(&[c("m", &["p", "s"]), c("p", &["a"]), c("s", &["a"]), c("a", &[])]);
        assert!(rows[0].is_merge);
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[0].branching, vec![1], "the second parent gets its own lane");
        assert_eq!(rows[1].lane, 0, "the first parent inherits the merge's lane");
        assert_eq!(rows[2].lane, 1, "the second parent is drawn in the lane opened for it");
        // Both sides then converge on `a`.
        assert_eq!(rows[3].lane, 0);
        assert_eq!(rows[3].closing, vec![1]);
    }

    #[test]
    fn a_merge_of_a_parent_something_already_heads_toward_reuses_that_lane() {
        // `m`'s second parent is `s`, which lane 1 is already waiting for.
        let rows = assign_lanes(&[c("t", &["s"]), c("m", &["p", "s"]), c("p", &["a"]), c("s", &["a"]), c("a", &[])]);
        let merge = &rows[1];
        assert!(merge.is_merge);
        assert_eq!(merge.branching.len(), 1);
        assert_eq!(merge.branching[0], 0, "reuses the lane already heading toward `s` instead of opening a second");
    }

    #[test]
    fn a_lane_is_reused_once_it_goes_idle() {
        // `y`'s lane frees up at the root `y`, so the later independent
        // tip `z` reuses it rather than opening a third column.
        let rows = assign_lanes(&[c("x", &["a"]), c("y", &[]), c("z", &["a"]), c("a", &[])]);
        assert_eq!(rows[1].lane, 1);
        assert_eq!(rows[2].lane, 1, "lane 1 went idle at `y` and is available again");
    }

    #[test]
    fn a_parent_dated_before_its_child_is_still_listed_after_it() {
        // `b`'s clock was behind: git lists its parent `a` first.
        let listed = vec![c("x", &["a"]), c("a", &["r"]), c("b", &["a"]), c("r", &[])];
        let order: Vec<String> = children_first(listed).into_iter().map(|c| c.hash).collect();
        assert_eq!(order, ["x", "b", "a", "r"]);
        // In order already: as given, parents outside the list ignored.
        let listed = vec![c("m", &["p", "s"]), c("p", &["gone"]), c("s", &["gone"])];
        let order: Vec<String> = children_first(listed).into_iter().map(|c| c.hash).collect();
        assert_eq!(order, ["m", "p", "s"]);
    }

    #[test]
    fn parse_line_reads_every_field_including_multiple_parents_and_refs() {
        let line = "abc123\x1fabc\x1fp1 p2\x1fHEAD -> main, origin/main\x1fJane\x1f2 days ago\x1fMerge branch 'x'";
        let commit = parse_line(line).unwrap();
        assert_eq!(commit.hash, "abc123");
        assert_eq!(commit.parents, vec!["p1".to_string(), "p2".to_string()]);
        assert_eq!(commit.refs, vec!["HEAD -> main".to_string(), "origin/main".to_string()]);
        assert_eq!(commit.author, "Jane");
        assert_eq!(commit.subject, "Merge branch 'x'");
    }

    #[test]
    fn parse_line_reads_a_root_commit_with_no_parents_and_no_refs() {
        let commit = parse_line("abc\x1fabc\x1f\x1f\x1fJane\x1fnow\x1finitial").unwrap();
        assert!(commit.parents.is_empty());
        assert!(commit.refs.is_empty());
    }

    #[test]
    fn commit_graph_is_empty_outside_a_git_repo() {
        let dir = TempDir::new("graph_no_repo");
        assert!(commit_graph(dir.path(), 50).is_empty());
    }

    #[test]
    fn commit_graph_sees_commits_on_every_branch_not_just_the_current_one() {
        let dir = TempDir::new("graph_all_branches");
        init_repo(dir.path());
        dir.write("a.txt", "v1");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        git(dir.path(), &["checkout", "-q", "-b", "side"]);
        dir.write("b.txt", "side work");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "on side"]);
        git(dir.path(), &["checkout", "-q", "main"]);

        // `git log` alone would only show `initial` from here.
        let commits = commit_graph(dir.path(), 50);
        assert_eq!(commits.len(), 2, "{commits:?}");
        assert!(commits.iter().any(|c| c.subject == "on side"));
        assert!(commits.iter().any(|c| c.refs.iter().any(|r| r.contains("side"))), "refs should name the branch: {commits:?}");
    }

    #[test]
    fn a_real_merge_commit_reports_two_parents_and_lands_as_a_merge_row() {
        let dir = TempDir::new("graph_real_merge");
        init_repo(dir.path());
        dir.write("a.txt", "v1");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        git(dir.path(), &["checkout", "-q", "-b", "side"]);
        dir.write("b.txt", "side");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "on side"]);
        git(dir.path(), &["checkout", "-q", "main"]);
        dir.write("c.txt", "main");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "on main"]);
        git(dir.path(), &["merge", "-q", "--no-ff", "-m", "merge side", "side"]);

        let commits = commit_graph(dir.path(), 50);
        let rows = assign_lanes(&commits);
        let merge_index = commits.iter().position(|c| c.subject == "merge side").expect("the merge commit is listed");
        assert_eq!(commits[merge_index].parents.len(), 2);
        assert!(rows[merge_index].is_merge);
        assert_eq!(rows[merge_index].branching.len(), 1, "the merged-in side gets a lane: {:?}", rows[merge_index]);
        assert_eq!(rows.len(), commits.len(), "one row per commit");
    }
}
