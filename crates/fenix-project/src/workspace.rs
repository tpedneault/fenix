//! Projects inside projects -- a monorepo. `find_project_root` answers
//! "which project is this file in" with the nearest one, which is right
//! for naming it, running its tasks and checking it. Two things need to
//! see further:
//!
//! - A language server wants the *language workspace* a project belongs
//!   to: one rust-analyzer for a Cargo workspace rather than one per
//!   member crate, and pyright started where a uv workspace's shared
//!   `.venv` is. `language_root` finds it.
//! - The hub wants to show what's inside a repository you registered:
//!   `subprojects` lists the projects below a root.

use std::path::{Path, PathBuf};

use crate::kind::{detect_kind, ProjectKind};

/// Whether `manifest` (a file's text) declares a workspace, for the
/// manifests that can: `Cargo.toml`'s `[workspace]`, `pyproject.toml`'s
/// `[tool.uv.workspace]`, `package.json`'s `"workspaces"`.
fn declares_workspace(file: &str, text: &str) -> bool {
    match file {
        "Cargo.toml" => text.lines().any(|l| l.trim() == "[workspace]"),
        "pyproject.toml" => text.lines().any(|l| l.trim() == "[tool.uv.workspace]"),
        "package.json" => text.contains("\"workspaces\""),
        _ => false,
    }
}

/// The manifests that make a workspace of each kind's projects.
fn workspace_manifests(kind: ProjectKind) -> &'static [&'static str] {
    match kind {
        ProjectKind::Rust => &["Cargo.toml"],
        ProjectKind::Python => &["pyproject.toml"],
        ProjectKind::Node => &["package.json", "pnpm-workspace.yaml"],
        ProjectKind::Go => &["go.work"],
        _ => &[],
    }
}

/// The language workspace `root` (a project root) is a member of -- the
/// nearest ancestor whose manifest of the same ecosystem declares one
/// (`go.work` and `pnpm-workspace.yaml` count by existing) -- else `root`
/// itself. Never looks past the repository `root` is in.
pub fn language_root(root: &Path) -> PathBuf {
    let kind = if root.join("Cargo.toml").is_file() {
        ProjectKind::Rust
    } else if root.join("pyproject.toml").is_file() {
        ProjectKind::Python
    } else if root.join("package.json").is_file() {
        ProjectKind::Node
    } else if root.join("go.mod").is_file() {
        ProjectKind::Go
    } else {
        return root.to_path_buf();
    };
    let boundary = crate::vcs::repository_root(root);
    root.parent().and_then(|parent| search(parent, kind, boundary.as_deref())).unwrap_or_else(|| root.to_path_buf())
}

/// The workspace a new project of `kind` created in `dir` would join --
/// `dir` itself or its nearest ancestor with a workspace manifest -- not
/// looking past the repository `dir` is in. `cargo init` and `uv init`
/// add themselves to it, so the wizard says so.
pub fn workspace_above(dir: &Path, kind: ProjectKind) -> Option<PathBuf> {
    search(dir, kind, crate::vcs::repository_root(dir).as_deref())
}

/// `start` and its ancestors, nearest first, for a workspace manifest of
/// `kind`'s ecosystem -- never outside `boundary`.
fn search(start: &Path, kind: ProjectKind, boundary: Option<&Path>) -> Option<PathBuf> {
    let ecosystem = workspace_manifests(kind);
    for dir in start.ancestors() {
        if boundary.is_some_and(|b| !dir.starts_with(b)) {
            break;
        }
        for file in ecosystem {
            let path = dir.join(file);
            let found = match *file {
                "go.work" | "pnpm-workspace.yaml" => path.is_file(),
                _ => std::fs::read_to_string(&path).is_ok_and(|text| declares_workspace(file, &text)),
            };
            if found {
                return Some(dir.to_path_buf());
            }
        }
    }
    None
}

/// Folders never worth looking inside for projects: build output,
/// dependencies, environments, tool state.
const SKIP: &[&str] = &["node_modules", "target", "build", "dist", "out", "venv", "__pycache__", "vendor", "third_party", "external"];

/// How deep `subprojects` looks, and how many it reports at most.
const MAX_DEPTH: usize = 4;
const MAX_FOUND: usize = 100;

/// The projects below `root` -- each folder under it (not `root` itself)
/// that `find_project_root` would call a project root -- with their
/// kinds, shallowest first, then by path. Skips hidden folders and the
/// usual build and dependency folders; stops at a few levels and a
/// hundred projects, so pointing it at a home folder stays cheap.
pub fn subprojects(root: &Path) -> Vec<(PathBuf, ProjectKind)> {
    let mut found = Vec::new();
    let mut level = vec![root.to_path_buf()];
    for _ in 0..MAX_DEPTH {
        let mut next = Vec::new();
        for dir in &level {
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            let mut children: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .filter(|p| p.file_name().map(|n| n.to_string_lossy()).is_some_and(|n| !n.starts_with('.') && !SKIP.contains(&n.as_ref())))
                .collect();
            children.sort();
            for child in children {
                if crate::find_project_root(&child).as_deref() == Some(child.as_path()) {
                    let kind = detect_kind(&child);
                    found.push((child.clone(), kind));
                    if found.len() >= MAX_FOUND {
                        return found;
                    }
                }
                next.push(child);
            }
        }
        level = next;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    fn monorepo() -> TempDir {
        let dir = TempDir::new("monorepo");
        dir.write(".git/HEAD", "ref: refs/heads/main\n");
        dir.write("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
        dir.write("crates/core/Cargo.toml", "[package]\nname = \"core\"\n");
        dir.write("crates/cli/Cargo.toml", "[package]\nname = \"cli\"\n");
        dir.write("tools/pyproject.toml", "[project]\nname='tools'\n[tool.uv.workspace]\nmembers = ['pkgs/*']\n");
        dir.write("tools/pkgs/decode/pyproject.toml", "[project]\nname='decode'\n");
        dir.write("labs/Blink/Blink.ino", "");
        dir.write("web/package.json", "{\"name\": \"web\"}");
        dir.write("web/node_modules/left-pad/package.json", "{}");
        dir.write("target/debug/build/x/Cargo.toml", "[package]");
        dir
    }

    #[test]
    fn members_share_their_workspace_as_language_root() {
        let dir = monorepo();
        assert_eq!(language_root(&dir.path().join("crates/core")), dir.path());
        assert_eq!(language_root(&dir.path().join("tools/pkgs/decode")), dir.path().join("tools"));
        assert_eq!(language_root(&dir.path().join("web")), dir.path().join("web"), "no npm workspace above it");
        assert_eq!(language_root(&dir.path().join("labs/Blink")), dir.path().join("labs/Blink"));
        assert_eq!(language_root(dir.path()), dir.path());
    }

    #[test]
    fn a_new_project_knows_the_workspace_it_would_join() {
        let dir = monorepo();
        assert_eq!(workspace_above(&dir.path().join("crates"), ProjectKind::Rust).as_deref(), Some(dir.path()));
        assert_eq!(workspace_above(&dir.path().join("tools/pkgs"), ProjectKind::Python).as_deref(), Some(dir.path().join("tools").as_path()));
        assert_eq!(workspace_above(&dir.path().join("labs"), ProjectKind::Arduino), None);
        assert_eq!(workspace_above(&dir.path().join("crates"), ProjectKind::Python), None);
    }

    #[test]
    fn a_workspace_is_never_looked_for_past_the_repository() {
        let outer = TempDir::new("monorepo_outer");
        outer.write("Cargo.toml", "[workspace]\n");
        outer.write("repo/.git/HEAD", "ref: refs/heads/main\n");
        outer.write("repo/Cargo.toml", "[package]\nname = \"alone\"\n");
        assert_eq!(language_root(&outer.path().join("repo")), outer.path().join("repo"));
    }

    #[test]
    fn subprojects_are_listed_shallowest_first_skipping_build_and_deps() {
        let dir = monorepo();
        let found: Vec<(String, ProjectKind)> = subprojects(dir.path())
            .into_iter()
            .map(|(p, k)| (p.strip_prefix(dir.path()).unwrap().to_string_lossy().replace('\\', "/"), k))
            .collect();
        assert_eq!(
            found,
            [
                ("tools".to_string(), ProjectKind::Python),
                ("web".to_string(), ProjectKind::Node),
                ("crates/cli".to_string(), ProjectKind::Rust),
                ("crates/core".to_string(), ProjectKind::Rust),
                ("labs/Blink".to_string(), ProjectKind::Arduino),
                ("tools/pkgs/decode".to_string(), ProjectKind::Python),
            ]
        );
    }
}
