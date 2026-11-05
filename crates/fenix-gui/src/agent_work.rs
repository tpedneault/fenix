//! The work behind Fenix's tools that needs nothing of the editor but
//! what it's handed: Git, project search, answers cut to size. Each runs
//! off the UI thread (`app/agent_calls.rs` hands it a root, the unsaved
//! buffers, the arguments) and answers JSON.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde_json::{json, Value};

/// `text`, at most `max` bytes, cut at a line, saying how to get the rest.
pub fn cap(text: &str, max: usize, rest: &str) -> Value {
    if text.len() <= max {
        return json!(text);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = &text[..end];
    let cut = cut.rfind('\n').map(|i| &cut[..i + 1]).unwrap_or(cut);
    json!(format!(
        "{cut}\n[cut at {} of {} bytes: {rest}]",
        cut.len(),
        text.len()
    ))
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

fn arg_usize(args: &Value, key: &str) -> Option<usize> {
    args.get(key).and_then(Value::as_u64).map(|n| n as usize)
}

/// The repository `root` is in, or an error saying it isn't one.
pub fn repo_of(root: &Path) -> Result<PathBuf, String> {
    root.ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{} isn't in a Git repository", root.display()))
}

/// `path` as Git names it: relative to the repository, with `/`.
pub fn repo_relative(repo: &Path, path: &str) -> String {
    let p = Path::new(path);
    let rel = if p.is_absolute() {
        p.strip_prefix(repo).unwrap_or(p)
    } else {
        p
    };
    rel.to_string_lossy().replace('\\', "/")
}

/// `git_status`.
pub fn git_status(repo: &Path) -> Value {
    let (status, files) = fenix_git::status_and_files(repo);
    let summary = fenix_git::summary(repo);
    let files: Vec<Value> = files
        .iter()
        .map(|f| {
            let state = |c: char| match c {
                'M' => "modified",
                'A' => "added",
                'D' => "deleted",
                'R' => "renamed",
                'C' => "copied",
                'U' => "conflicted",
                '?' => "untracked",
                _ => "",
            };
            json!({ "path": f.path, "staged": state(f.index_status), "unstaged": state(f.worktree_status) })
        })
        .collect();
    json!({
        "branch": status.as_ref().map(|s| s.branch.clone()),
        "upstream": status.as_ref().and_then(|s| s.upstream.clone()),
        "ahead": status.as_ref().map(|s| s.ahead),
        "behind": status.as_ref().map(|s| s.behind),
        "head": summary.as_ref().and_then(|s| s.head.clone()),
        "conflicts": summary.as_ref().map(|s| s.conflicts).unwrap_or(0),
        "files": files,
    })
}

/// `git_diff`: a file's or the whole tree's, unstaged by default, staged
/// with `staged`, or a branch's whole change against `base`.
pub fn git_diff(repo: &Path, args: &Value, max: usize) -> Result<Value, String> {
    let path = arg_str(args, "path").map(|p| repo_relative(repo, p));
    let text = if let Some(base) = arg_str(args, "base") {
        let head = arg_str(args, "head").unwrap_or("HEAD");
        match &path {
            Some(path) => fenix_git::run_args(
                repo,
                &[
                    "diff",
                    "--no-color",
                    &format!("{base}...{head}"),
                    "--",
                    path,
                ],
            )?,
            None => fenix_git::diff_refs(repo, base, head, true)?,
        }
    } else {
        let staged = args.get("staged").and_then(Value::as_bool).unwrap_or(false);
        fenix_git::file_diff(repo, path.as_deref().unwrap_or("."), staged)?
    };
    let rest = if path.is_some() {
        "ask for a smaller range of the file"
    } else {
        "ask for one path at a time"
    };
    Ok(
        json!({ "diff": if text.is_empty() { json!("(no changes)") } else { cap(&text, max, rest) } }),
    )
}

/// `git_log`.
pub fn git_log(repo: &Path, args: &Value) -> Result<Value, String> {
    let max = arg_usize(args, "max").unwrap_or(20).clamp(1, 200);
    if let (Some(path), Some(start)) = (arg_str(args, "path"), arg_usize(args, "startLine")) {
        let end = arg_usize(args, "endLine").unwrap_or(start);
        let commits = fenix_git::line_log(repo, &repo_relative(repo, path), start, end, max)?;
        return Ok(
            json!({ "commits": commits.iter().map(|(c, _)| commit_json(c)).collect::<Vec<_>>() }),
        );
    }
    if let Some(range) = arg_str(args, "range") {
        let text = fenix_git::run_args(
            repo,
            &[
                "log",
                "--no-color",
                &format!("--max-count={max}"),
                "--format=%H%x1f%an%x1f%ar%x1f%s",
                range,
            ],
        )?;
        let commits: Vec<Value> = text
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\x1f').collect();
                (f.len() == 4)
                    .then(|| json!({ "hash": f[0], "author": f[1], "when": f[2], "subject": f[3] }))
            })
            .collect();
        return Ok(json!({ "commits": commits }));
    }
    let query = fenix_git::LogQuery {
        all: false,
        path: arg_str(args, "path").map(|p| repo_relative(repo, p)),
        grep: arg_str(args, "grep").map(str::to_string),
        author: None,
        limit: max,
    };
    Ok(
        json!({ "commits": fenix_git::log(repo, &query).iter().map(commit_json).collect::<Vec<_>>() }),
    )
}

fn commit_json(c: &fenix_git::GraphCommit) -> Value {
    json!({ "hash": c.hash, "author": c.author, "when": c.relative_date, "subject": c.subject, "refs": c.refs })
}

/// `git_show`: a commit's message and diff, or one file of it.
pub fn git_show(repo: &Path, args: &Value, max: usize) -> Result<Value, String> {
    let commit = arg_str(args, "commit").ok_or("commit: which one (a hash or a ref)")?;
    let text = match arg_str(args, "path") {
        Some(path) => fenix_git::commit_file_diff(repo, commit, &repo_relative(repo, path))?,
        None => fenix_git::commit_diff(repo, commit)?,
    };
    let files: Vec<Value> = fenix_git::commit_files(repo, commit)
        .into_iter()
        .map(|(status, path)| json!({ "status": status.to_string(), "path": path }))
        .collect();
    Ok(json!({ "files": files, "show": cap(&text, max, "ask for one path of the commit") }))
}

/// `git_blame`: who last changed each line, `contents` being the
/// buffer's text when it's open (unsaved lines say so).
pub fn git_blame(repo: &Path, args: &Value, contents: Option<&str>) -> Result<Value, String> {
    let path = arg_str(args, "path").ok_or("path: which file")?;
    let lines = fenix_git::blame(repo, &repo_relative(repo, path), contents)?;
    let start = arg_usize(args, "startLine").unwrap_or(1).max(1);
    let end = arg_usize(args, "endLine")
        .unwrap_or(start + 99)
        .min(lines.len())
        .max(start);
    let out: Vec<Value> = lines
        .iter()
        .enumerate()
        .skip(start - 1)
        .take(end + 1 - start)
        .map(|(i, l)| if l.uncommitted() { json!({ "line": i + 1, "uncommitted": true }) } else { json!({ "line": i + 1, "commit": &l.hash[..l.hash.len().min(12)], "author": l.author, "time": l.time, "summary": l.summary }) })
        .collect();
    Ok(json!({ "lines": out, "of": lines.len() }))
}

/// `search`: the project's files, unsaved text included, its ignores
/// respected.
pub fn search(
    root: &Path,
    unsaved: &HashMap<PathBuf, String>,
    args: &Value,
) -> Result<Value, String> {
    let query = arg_str(args, "pattern").ok_or("pattern: what to search for")?;
    let options = fenix_project::search::Options {
        match_case: args.get("case").and_then(Value::as_bool).unwrap_or(false),
        whole_word: args.get("word").and_then(Value::as_bool).unwrap_or(false),
        regex: args.get("regex").and_then(Value::as_bool).unwrap_or(false),
        hidden: false,
        globs: args
            .get("glob")
            .and_then(Value::as_str)
            .map(|g| {
                g.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        context: 0,
    };
    let pattern = fenix_project::search::Pattern::new(query, &options)?;
    let max = arg_usize(args, "max").unwrap_or(100).clamp(1, 1000);
    let mut matches = Vec::new();
    let cancel = AtomicBool::new(false);
    let summary = fenix_project::search::search(
        &fenix_project::search::Scope::Folders(vec![root.to_path_buf()]),
        &pattern,
        &options,
        unsaved,
        max,
        &cancel,
        |file| {
            for line in &file.lines {
                if !line.matches.is_empty() {
                    matches.push(json!({ "path": file.path.to_string_lossy(), "line": line.number, "text": line.text.trim_end() }));
                }
            }
            if matches.len() >= max {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        },
    )?;
    matches.truncate(max);
    let cut = summary.truncated || matches.len() >= max;
    Ok(json!({ "matches": matches, "filesSearched": summary.files_searched, "cut": cut }))
}

/// `replace` worked out: for each file with a match, its text after the
/// replacement and how many were replaced. Nothing is written here.
pub fn replacements(
    root: &Path,
    unsaved: &HashMap<PathBuf, String>,
    args: &Value,
) -> Result<Vec<(PathBuf, String, usize)>, String> {
    let query = arg_str(args, "pattern").ok_or("pattern: what to replace")?;
    let replacement = args
        .get("replacement")
        .and_then(Value::as_str)
        .ok_or("replacement: what with")?;
    let options = fenix_project::search::Options {
        match_case: args.get("case").and_then(Value::as_bool).unwrap_or(true),
        whole_word: args.get("word").and_then(Value::as_bool).unwrap_or(false),
        regex: args.get("regex").and_then(Value::as_bool).unwrap_or(false),
        hidden: false,
        globs: args
            .get("glob")
            .and_then(Value::as_str)
            .map(|g| {
                g.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        context: 0,
    };
    let preserve = args
        .get("preserve_case")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let pattern = fenix_project::search::Pattern::new(query, &options)?;
    let mut files = Vec::new();
    let cancel = AtomicBool::new(false);
    fenix_project::search::search(
        &fenix_project::search::Scope::Folders(vec![root.to_path_buf()]),
        &pattern,
        &options,
        unsaved,
        100_000,
        &cancel,
        |file| {
            files.push(file.path.clone());
            ControlFlow::Continue(())
        },
    )?;
    let mut out = Vec::new();
    for path in files {
        let text = match unsaved.get(&path) {
            Some(text) => text.clone(),
            None => {
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?
            }
        };
        let (new, count) =
            fenix_project::search::replace(&text, &pattern, replacement, preserve, &|_| false);
        if count > 0 {
            out.push((path, new, count));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        git(dir.path(), &["config", "user.email", "t@example.com"]);
        git(dir.path(), &["config", "user.name", "T"]);
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "First"]);
        dir
    }

    #[test]
    fn a_long_answer_is_cut_at_a_line_and_says_so() {
        assert_eq!(cap("short", 100, "x"), json!("short"));
        let text = "line one\nline two\nline three\n";
        let cut = cap(text, 15, "ask for less").as_str().unwrap().to_string();
        assert!(
            cut.starts_with("line one\n\n[cut at 9 of 29 bytes: ask for less]"),
            "{cut}"
        );
    }

    #[test]
    fn status_diff_log_show_and_blame_read_the_repository() {
        let dir = repo();
        let repo = repo_of(dir.path()).unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\nfn c() {}\n").unwrap();
        let status = git_status(&repo);
        assert_eq!(status["branch"], "main");
        assert_eq!(
            status["files"][0],
            json!({ "path": "a.rs", "staged": "", "unstaged": "modified" })
        );
        let diff = git_diff(
            &repo,
            &json!({ "path": dir.path().join("a.rs").to_string_lossy() }),
            64 * 1024,
        )
        .unwrap();
        assert!(
            diff["diff"].as_str().unwrap().contains("+fn c() {}"),
            "{diff}"
        );
        assert_eq!(
            git_diff(&repo, &json!({ "staged": true }), 1024).unwrap()["diff"],
            "(no changes)"
        );
        let log = git_log(&repo, &json!({})).unwrap();
        assert_eq!(log["commits"][0]["subject"], "First");
        let hash = log["commits"][0]["hash"].as_str().unwrap().to_string();
        let show = git_show(&repo, &json!({ "commit": hash }), 64 * 1024).unwrap();
        assert_eq!(show["files"][0]["path"], "a.rs");
        let blame = git_blame(
            &repo,
            &json!({ "path": "a.rs" }),
            Some("fn a() {}\nfn c() {}\n"),
        )
        .unwrap();
        assert_eq!(blame["lines"][0]["summary"], "First");
        assert_eq!(
            blame["lines"][1]["uncommitted"], true,
            "the buffer's unsaved line says so"
        );
    }

    #[test]
    fn outside_a_repository_says_so() {
        let dir = tempfile::tempdir().unwrap();
        assert!(repo_of(dir.path())
            .unwrap_err()
            .contains("isn't in a Git repository"));
    }

    #[test]
    fn search_sees_unsaved_text_and_replace_works_it_out_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        std::fs::write(&a, "old name\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "the old name again\n").unwrap();
        let unsaved = HashMap::from([(a.clone(), "new name, unsaved\n".to_string())]);
        let found = search(dir.path(), &unsaved, &json!({ "pattern": "name" })).unwrap();
        let texts: Vec<&str> = found["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["text"].as_str().unwrap())
            .collect();
        assert!(
            texts.contains(&"new name, unsaved") && texts.contains(&"the old name again"),
            "{texts:?}"
        );
        let changes = replacements(
            dir.path(),
            &unsaved,
            &json!({ "pattern": "old", "replacement": "new" }),
        )
        .unwrap();
        assert_eq!(changes.len(), 1, "a.txt's unsaved text has no 'old' left");
        assert_eq!(changes[0].1, "the new name again\n");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "the old name again\n",
            "nothing written"
        );
        assert!(search(dir.path(), &unsaved, &json!({})).is_err());
    }
}
