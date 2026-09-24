//! Interactive rebase without an editor: the caller says what to do with
//! each commit, this writes the todo list git would have opened and runs
//! the rebase with that list put in place -- so git does all the work.

use std::path::{Path, PathBuf};

use crate::process::{run_action_env, run_lines};

/// What to do with one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Pick,
    /// Keep the commit with this message instead.
    Reword(String),
    /// Stop after it, to amend it.
    Edit,
    /// Fold into the commit before, keeping both messages.
    Squash,
    /// Fold into the commit before, dropping this message.
    Fixup,
    Drop,
}

/// One line of the plan: a commit, oldest first, and what happens to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub hash: String,
    pub step: Step,
}

/// A commit a rebase would replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replayed {
    pub hash: String,
    pub short_hash: String,
    pub subject: String,
}

/// The commits `git rebase -i base` would list, oldest first -- refused
/// when the range holds a merge, which a plain todo list would flatten.
pub fn replayed(repo: &Path, base: Option<&str>) -> Result<Vec<Replayed>, String> {
    let range = match base {
        Some(base) => format!("{base}..HEAD"),
        None => "HEAD".to_string(),
    };
    if !run_lines(repo, &["rev-list", "--merges", "-n1", &range]).is_empty() {
        return Err("there's a merge in that range -- rebase it by hand with --rebase-merges".to_string());
    }
    Ok(run_lines(repo, &["log", "--reverse", "--format=%H\x1f%h\x1f%s", &range])
        .into_iter()
        .filter_map(|l| {
            let mut f = l.splitn(3, '\x1f');
            Some(Replayed { hash: f.next()?.to_string(), short_hash: f.next()?.to_string(), subject: f.next().unwrap_or("").to_string() })
        })
        .collect())
}

/// The todo list for `plan`: one line per commit, and after a reword the
/// command that gives it its new message from a file (`messages` holds
/// each reword's file, by commit).
pub fn todo(plan: &[Planned], messages: &dyn Fn(&str) -> Option<PathBuf>) -> String {
    let mut out = String::new();
    for p in plan {
        let verb = match p.step {
            Step::Pick | Step::Reword(_) => "pick",
            Step::Edit => "edit",
            Step::Squash => "squash",
            Step::Fixup => "fixup",
            Step::Drop => "drop",
        };
        out.push_str(&format!("{verb} {}\n", p.hash));
        if let (Step::Reword(_), Some(file)) = (&p.step, messages(&p.hash)) {
            out.push_str(&format!("exec git commit --amend --only --allow-empty -F '{}'\n", slashes(&file)));
        }
    }
    out
}

/// A path the way git's own shell takes it on every platform.
fn slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Runs the rebase onto `base` (the root, when `None`) doing `plan`.
/// Stopping for an `edit` or a conflict comes back as `Err` with the
/// rebase in progress, like any other rebase here.
pub fn rebase_interactive(repo: &Path, base: Option<&str>, plan: &[Planned]) -> Result<String, String> {
    let dir = run_lines(repo, &["rev-parse", "--absolute-git-dir"]).into_iter().next().ok_or("not a git repository")?;
    let dir = PathBuf::from(dir).join("fenix").join("rebase");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    for p in plan {
        if let Step::Reword(message) = &p.step {
            let file = dir.join(format!("message-{}", &p.hash[..12.min(p.hash.len())]));
            std::fs::write(&file, message).map_err(|e| e.to_string())?;
            files.push((p.hash.clone(), file));
        }
    }
    let list = todo(plan, &|hash| files.iter().find(|(h, _)| h == hash).map(|(_, f)| f.clone()));
    let todo_file = dir.join("todo");
    std::fs::write(&todo_file, list).map_err(|e| e.to_string())?;
    let editor = format!("cp '{}'", slashes(&todo_file));
    let mut args = vec!["rebase".to_string(), "-i".to_string(), "--autostash".to_string()];
    match base {
        Some(base) => args.push(base.to_string()),
        None => args.push("--root".to_string()),
    }
    run_action_env(repo, &args, &[("GIT_SEQUENCE_EDITOR", &editor)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{git, init_repo, TempDir};

    fn repo(name: &str) -> TempDir {
        let dir = TempDir::new(name);
        init_repo(dir.path());
        git(dir.path(), &["config", "core.autocrlf", "false"]);
        for (file, message) in [("a.txt", "first"), ("b.txt", "second"), ("c.txt", "third"), ("d.txt", "fourth")] {
            dir.write(file, message);
            git(dir.path(), &["add", "."]);
            git(dir.path(), &["commit", "-q", "-m", message]);
        }
        dir
    }

    fn log(dir: &Path) -> Vec<String> {
        run_lines(dir, &["log", "--format=%s"])
    }

    #[test]
    fn the_replayed_commits_come_oldest_first() {
        let dir = repo("rebase_list");
        let base = run_lines(dir.path(), &["rev-parse", "HEAD~3"]).remove(0);
        let list = replayed(dir.path(), Some(&base)).unwrap();
        assert_eq!(list.iter().map(|r| r.subject.as_str()).collect::<Vec<_>>(), ["second", "third", "fourth"]);
    }

    #[test]
    fn a_plan_reorders_rewords_squashes_and_drops() {
        let dir = repo("rebase_plan");
        let base = run_lines(dir.path(), &["rev-parse", "HEAD~3"]).remove(0);
        let list = replayed(dir.path(), Some(&base)).unwrap();
        let (second, third, fourth) = (list[0].hash.clone(), list[1].hash.clone(), list[2].hash.clone());
        let plan = vec![
            Planned { hash: fourth, step: Step::Reword("fourth, reworded\n\nwith a body".into()) },
            Planned { hash: second, step: Step::Pick },
            Planned { hash: third, step: Step::Fixup },
        ];
        rebase_interactive(dir.path(), Some(&base), &plan).unwrap();
        assert_eq!(log(dir.path()), ["second", "fourth, reworded", "first"]);
        assert_eq!(run_lines(dir.path(), &["log", "-1", "--format=%B", "HEAD~1"]).join("\n").trim(), "fourth, reworded\n\nwith a body");
        assert!(dir.path().join("c.txt").exists(), "third's change was folded into second, not lost");
    }

    #[test]
    fn a_dropped_commit_is_gone() {
        let dir = repo("rebase_drop");
        let base = run_lines(dir.path(), &["rev-parse", "HEAD~2"]).remove(0);
        let list = replayed(dir.path(), Some(&base)).unwrap();
        let plan = vec![Planned { hash: list[0].hash.clone(), step: Step::Drop }, Planned { hash: list[1].hash.clone(), step: Step::Pick }];
        rebase_interactive(dir.path(), Some(&base), &plan).unwrap();
        assert_eq!(log(dir.path()), ["fourth", "second", "first"]);
        assert!(!dir.path().join("c.txt").exists());
    }

    #[test]
    fn an_edit_stops_the_rebase_there() {
        let dir = repo("rebase_edit");
        let base = run_lines(dir.path(), &["rev-parse", "HEAD~2"]).remove(0);
        let list = replayed(dir.path(), Some(&base)).unwrap();
        let plan = vec![Planned { hash: list[0].hash.clone(), step: Step::Edit }, Planned { hash: list[1].hash.clone(), step: Step::Pick }];
        let _ = rebase_interactive(dir.path(), Some(&base), &plan);
        assert!(matches!(crate::state::in_progress(dir.path()), Some(crate::InProgress::Rebase { .. })));
        crate::actions::rebase_continue(dir.path()).unwrap();
        assert_eq!(log(dir.path())[0], "fourth");
    }

    #[test]
    fn a_merge_in_the_range_is_refused() {
        let dir = repo("rebase_merge");
        git(dir.path(), &["switch", "-q", "-c", "side", "HEAD~1"]);
        dir.write("e.txt", "e");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "side"]);
        git(dir.path(), &["switch", "-q", "main"]);
        git(dir.path(), &["merge", "-q", "--no-edit", "side"]);
        let base = run_lines(dir.path(), &["rev-parse", "HEAD~3"]).remove(0);
        assert!(replayed(dir.path(), Some(&base)).unwrap_err().contains("merge"));
    }
}
