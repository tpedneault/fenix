use std::path::Path;

use crate::process::run_lines;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stash {
    /// The `N` in `stash@{N}` -- what every stash action (`apply`/`pop`/
    /// `drop`) needs to target the right entry.
    pub index: usize,
    pub message: String,
}

pub fn list_stashes(repo: &Path) -> Vec<Stash> {
    run_lines(repo, &["stash", "list", "--format=%gd\x1f%s"]).iter().filter_map(|l| parse_line(l)).collect()
}

fn parse_line(line: &str) -> Option<Stash> {
    let (gd, message) = line.split_once('\x1f')?;
    let index = gd.strip_prefix("stash@{")?.strip_suffix('}')?.parse().ok()?;
    Some(Stash { index, message: message.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{git, init_repo, TempDir};

    #[test]
    fn parse_line_reads_the_index_and_message() {
        let stash = parse_line("stash@{0}\x1fWIP on main: abc123 initial").unwrap();
        assert_eq!(stash, Stash { index: 0, message: "WIP on main: abc123 initial".to_string() });
    }

    #[test]
    fn list_stashes_empty_outside_a_git_repo() {
        let dir = TempDir::new("stash_no_repo");
        assert!(list_stashes(dir.path()).is_empty());
    }

    #[test]
    fn list_stashes_empty_with_nothing_stashed() {
        let dir = TempDir::new("stash_empty");
        init_repo(dir.path());
        assert!(list_stashes(dir.path()).is_empty());
    }

    #[test]
    fn list_stashes_reads_a_real_stash_with_a_custom_message() {
        let dir = TempDir::new("stash_real");
        init_repo(dir.path());
        dir.write("a.txt", "v1");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        dir.write("a.txt", "v2");
        git(dir.path(), &["stash", "push", "-m", "my custom stash"]);

        let stashes = list_stashes(dir.path());
        assert_eq!(stashes.len(), 1);
        assert_eq!(stashes[0].index, 0);
        assert!(stashes[0].message.contains("my custom stash"));
    }
}
