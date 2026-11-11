//! The test a line of a file is in, as a task that runs only it: `SPC p r`
//! in the editor, and what an agent's `run_test {path, line}` asks for.
//!
//! Read from the text, not a language server: a Rust `#[test]` function
//! (its module path from the file's place in the crate, and the `mod`s
//! around it), a pytest `test_*` function or `Test*` class, or a Go
//! `func TestXxx`. Each runs through the tool the project's own test task
//! uses (`cargo test`, `pytest`, `go test`), in the project's root.

use std::path::{Path, PathBuf};

use crate::TaskDef;

/// The task that runs the test at `line` (1-based) of `path`, whose text
/// is `text` (unsaved edits included), from the project at `root`. Says
/// why when there's none: the line isn't in a test, or tests in that
/// kind of file can't be run one at a time yet.
pub fn test_at(path: &Path, text: &str, line: usize, root: &Path) -> Result<TaskDef, String> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Err(format!("{} is empty", path.display()));
    }
    let at = line.max(1).min(lines.len()) - 1;
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
    match extension {
        "rs" => rust_test(path, &lines, at),
        "py" => python_test(path, &lines, at, root),
        "go" => go_test(path, &lines, at, root),
        _ => Err(format!("running one test isn't known for .{extension} files yet -- SPC p t runs the project's tests")),
    }
}

/// The lines (0-based) of `text` (the file at `path`, of the project at
/// `root`) that declare a test `test_at` can run alone: where Fenix's own
/// "Run test" lens goes.
pub fn test_lines(path: &Path, text: &str, root: &Path) -> Vec<usize> {
    let lines: Vec<&str> = text.lines().collect();
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
    (0..lines.len())
        .filter(|&i| match extension {
            "rs" => rust_fn_name(lines[i]).is_some() && rust_is_test(&lines, i),
            "py" => (python_def(lines[i], "def").is_some() || python_def(lines[i], "class").is_some()) && python_test(path, &lines, i, root).is_ok(),
            "go" => lines[i].starts_with("func Test") && go_test(path, &lines, i, root).is_ok(),
            _ => false,
        })
        .collect()
}

fn no_test(path: &Path, at: usize) -> String {
    format!("line {} of {} isn't in a test", at + 1, file_name(path))
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// `path` from `root`, with `/` between its parts, when it's under it;
/// else as it is.
fn relative(path: &Path, root: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

// ---- Rust ----

/// The name `line` declares a function by: `fn name(`, after any of
/// `pub`, `pub(crate)`, `async`, `const`, `unsafe`, `extern "C"`.
fn rust_fn_name(line: &str) -> Option<&str> {
    let code = line.trim_start();
    let at = code.find("fn ")?;
    let before = &code[..at];
    if !before.split_whitespace().all(|w| w == "pub" || w.starts_with("pub(") || matches!(w, "async" | "const" | "unsafe" | "extern" | "\"C\"" | "\"Rust\"")) {
        return None;
    }
    let rest = &code[at + 3..];
    let name: &str = &rest[..rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len())];
    (!name.is_empty() && rest[name.len()..].trim_start().starts_with(['(', '<'])).then_some(name)
}

/// Whether the attributes above line `fn_line` mark a test: `#[test]`,
/// `#[tokio::test]`, `#[rstest]`, `#[test_case(..)]` and the like.
fn rust_is_test(lines: &[&str], fn_line: usize) -> bool {
    for line in lines[..fn_line].iter().rev() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if !line.starts_with("#[") && !line.ends_with(")]") && !line.ends_with(',') && !line.ends_with('(') {
            return false;
        }
        let attribute = line.trim_start_matches("#[");
        let path = &attribute[..attribute.find(['(', ']']).unwrap_or(attribute.len())];
        if path == "test" || path.ends_with("::test") || path == "rstest" || path.starts_with("test_case") || path == "quickcheck" {
            return true;
        }
    }
    false
}

/// The `mod`s line `upto` is inside, outermost first, from the braces
/// before it (strings, characters and comments skipped).
fn rust_inline_mods(lines: &[&str], upto: usize) -> Vec<String> {
    let mut depth = 0usize;
    let mut mods: Vec<(String, usize)> = Vec::new();
    let mut in_block_comment = false;
    for line in &lines[..upto] {
        let code = line.trim_start();
        let opens_mod = code
            .split_whitespace()
            .skip_while(|w| *w == "pub" || w.starts_with("pub("))
            .collect::<Vec<_>>();
        let mod_name = match opens_mod.as_slice() {
            ["mod", name, ..] if line.contains('{') => Some(name.trim_end_matches('{').to_string()),
            _ => None,
        };
        let before = depth;
        let mut chars = line.chars().peekable();
        let mut in_string = false;
        while let Some(c) = chars.next() {
            if in_block_comment {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    in_block_comment = false;
                }
                continue;
            }
            if in_string {
                match c {
                    '\\' => {
                        chars.next();
                    }
                    '"' => in_string = false,
                    _ => {}
                }
                continue;
            }
            match c {
                '/' if chars.peek() == Some(&'/') => break,
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    in_block_comment = true;
                }
                '"' => in_string = true,
                // A character literal: '{', '\'' -- not a lifetime.
                '\'' => {
                    let rest: String = chars.clone().take(3).collect();
                    if rest.starts_with('\\') {
                        chars.next();
                        chars.next();
                        chars.next();
                    } else if rest.chars().nth(1) == Some('\'') {
                        chars.next();
                        chars.next();
                    }
                }
                '{' => depth += 1,
                '}' => {
                    depth = depth.saturating_sub(1);
                    while mods.last().is_some_and(|(_, at)| depth <= *at) {
                        mods.pop();
                    }
                }
                _ => {}
            }
        }
        if let Some(name) = mod_name {
            if depth > before {
                mods.push((name, before));
            }
        }
    }
    mods.into_iter().map(|(name, _)| name).collect()
}

/// The nearest folder at or above `path`'s with a `Cargo.toml` in it, and
/// its package's name.
fn rust_package(path: &Path) -> Option<(PathBuf, String)> {
    let mut dir = path.parent();
    while let Some(here) = dir {
        if let Ok(text) = std::fs::read_to_string(here.join("Cargo.toml")) {
            let mut in_package = false;
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    in_package = line == "[package]";
                } else if in_package {
                    if let Some(value) = line.strip_prefix("name").map(str::trim_start).and_then(|l| l.strip_prefix('=')) {
                        return Some((here.to_path_buf(), value.trim().trim_matches('"').to_string()));
                    }
                }
            }
        }
        dir = here.parent();
    }
    None
}

/// What `cargo test` takes to pick `path`'s test binary (`--test x`,
/// `--bin x`, or nothing for the library's and main's), and the module
/// path its file is at in that binary.
fn rust_target_and_module(crate_dir: &Path, path: &Path) -> (Vec<String>, Vec<String>) {
    let rel: Vec<String> = path.strip_prefix(crate_dir).unwrap_or(path).components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let stem = |name: &str| name.strip_suffix(".rs").unwrap_or(name).to_string();
    match rel.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["tests", file] => (vec!["--test".into(), stem(file)], Vec::new()),
        ["tests", dir, rest @ ..] => {
            let mut module: Vec<String> = rest.iter().map(|p| stem(p)).collect();
            if matches!(module.last().map(String::as_str), Some("main" | "mod")) {
                module.pop();
            }
            (vec!["--test".into(), dir.to_string()], module)
        }
        ["src", "bin", file] => (vec!["--bin".into(), stem(file)], Vec::new()),
        ["src", "bin", dir, rest @ ..] => {
            let mut module: Vec<String> = rest.iter().map(|p| stem(p)).collect();
            if matches!(module.last().map(String::as_str), Some("main" | "mod")) {
                module.pop();
            }
            (vec!["--bin".into(), dir.to_string()], module)
        }
        ["src", rest @ ..] => {
            let mut module: Vec<String> = rest.iter().map(|p| stem(p)).collect();
            if module.len() == 1 && matches!(module[0].as_str(), "lib" | "main") {
                module.clear();
            }
            if module.last().map(String::as_str) == Some("mod") {
                module.pop();
            }
            (Vec::new(), module)
        }
        _ => (Vec::new(), Vec::new()),
    }
}

fn rust_test(path: &Path, lines: &[&str], at: usize) -> Result<TaskDef, String> {
    // On a test's attributes, that test; otherwise the function the
    // line is in.
    let on_attribute = (at..lines.len()).take_while(|&i| lines[i].trim_start().starts_with("#[")).last().map(|i| i + 1).filter(|&i| i < lines.len() && rust_fn_name(lines[i]).is_some());
    let fn_line = on_attribute.or_else(|| (0..=at).rev().find(|&i| rust_fn_name(lines[i]).is_some())).ok_or_else(|| no_test(path, at))?;
    if !rust_is_test(lines, fn_line) {
        return Err(no_test(path, at));
    }
    let name = rust_fn_name(lines[fn_line]).unwrap_or_default();
    let (crate_dir, package) = rust_package(path).ok_or_else(|| format!("{} isn't in a Cargo package", file_name(path)))?;
    let (target, mut module) = rust_target_and_module(&crate_dir, path);
    module.extend(rust_inline_mods(lines, fn_line));
    module.push(name.to_string());
    let filter = module.join("::");
    let mut args = vec!["test".to_string(), "--message-format=json".into(), "-p".into(), package];
    args.extend(target);
    args.extend(["--".into(), filter.clone(), "--exact".into()]);
    Ok(TaskDef { name: format!("test {filter}"), command: "cargo".into(), args })
}

// ---- Python ----

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// `def name(` or `async def name(`, and `class Name` -- the name.
fn python_def(line: &str, keyword: &str) -> Option<String> {
    let code = line.trim_start();
    let code = if keyword == "def" { code.strip_prefix("async ").unwrap_or(code) } else { code };
    let rest = code.strip_prefix(keyword)?.strip_prefix(' ')?;
    let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    (!name.is_empty()).then_some(name)
}

fn python_test(path: &Path, lines: &[&str], at: usize, root: &Path) -> Result<TaskDef, String> {
    // The `def`s and `class`es the line is in, innermost first: walking
    // up, each line indented less than everything since is the block
    // around them.
    let mut node: Vec<(bool, String)> = Vec::new();
    let mut limit = usize::MAX;
    for line in lines[..=at].iter().rev() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indent = indent_of(line);
        if indent >= limit {
            continue;
        }
        limit = indent;
        if let Some(name) = python_def(line, "def") {
            // A function inside another: the outer one is what runs.
            node.retain(|(is_class, _)| *is_class);
            node.push((false, name));
        } else if let Some(name) = python_def(line, "class") {
            node.push((true, name));
        }
        if indent == 0 {
            break;
        }
    }
    let is_test = |(is_class, name): &(bool, String)| if *is_class { name.starts_with("Test") } else { name.starts_with("test") };
    if node.is_empty() || !node.iter().all(is_test) {
        return Err(no_test(path, at));
    }
    let names: Vec<String> = node.into_iter().rev().map(|(_, name)| name).collect();
    let id = format!("{}::{}", relative(path, root), names.join("::"));
    Ok(TaskDef { name: format!("test {}", names.join("::")), command: "pytest".into(), args: vec![id] })
}

// ---- Go ----

fn go_test(path: &Path, lines: &[&str], at: usize, root: &Path) -> Result<TaskDef, String> {
    let name = (0..=at)
        .rev()
        .find_map(|i| lines[i].strip_prefix("func ").map(|rest| rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect::<String>()).filter(|n| !n.is_empty()))
        .ok_or_else(|| no_test(path, at))?;
    if !["Test", "Benchmark", "Example", "Fuzz"].iter().any(|p| name.starts_with(p)) {
        return Err(no_test(path, at));
    }
    let dir = path.parent().map(|d| relative(d, root)).unwrap_or_default();
    let package = if dir.is_empty() { ".".to_string() } else if Path::new(&dir).is_absolute() { dir } else { format!("./{dir}") };
    Ok(TaskDef { name: format!("test {name}"), command: "go".into(), args: vec!["test".into(), "-run".into(), format!("^{name}$"), package] })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("fenix-tasks-test-at-{name}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn write(&self, rel: &str, text: &str) -> PathBuf {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_file_s_test_lines_are_those_declaring_one() {
        let rust = "fn helper() {}\n\n#[test]\nfn adds() {}\n\n#[cfg(test)]\nmod tests {\n    #[tokio::test]\n    async fn waits() {}\n}\n";
        assert_eq!(test_lines(Path::new("/p/src/lib.rs"), rust, Path::new("/p")), [3, 8]);
        let py = "def helper():\n    pass\n\ndef test_adds():\n    pass\n\nclass TestThings:\n    def test_one(self):\n        pass\n";
        assert_eq!(test_lines(Path::new("/p/test_x.py"), py, Path::new("/p")), [3, 6, 7]);
        let go = "package x\n\nfunc Add() {}\n\nfunc TestAdd(t *testing.T) {}\n";
        assert_eq!(test_lines(Path::new("/p/x_test.go"), go, Path::new("/p")), [4]);
        assert!(test_lines(Path::new("/p/a.txt"), "x", Path::new("/p")).is_empty());
    }

    const LIB: &str = r#"pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn helper() -> char {
        '{'
    }

    #[test]
    fn adds() {
        let s = "}{";
        assert_eq!(add(1, 2), 3);
    }

    mod nested {
        #[tokio::test]
        async fn deeper() {
            assert!(true);
        }
    }

    #[test]
    fn after_nested() {}
}
"#;

    fn crate_with(dir: &TempDir, rel: &str, text: &str) -> PathBuf {
        dir.write("Cargo.toml", "[package]\nname = \"ledger\"\nversion = \"0.1.0\"\n\n[dependencies]\nname-like = \"1\"\n");
        dir.write(rel, text)
    }

    #[test]
    fn a_rust_test_runs_by_its_whole_path() {
        let dir = TempDir::new("rust");
        let path = crate_with(&dir, "src/amounts/parse.rs", LIB);
        // On the assert inside `adds`.
        let task = test_at(&path, LIB, 16, &dir.0).unwrap();
        assert_eq!(task.command, "cargo");
        assert_eq!(task.args, ["test", "--message-format=json", "-p", "ledger", "--", "amounts::parse::tests::adds", "--exact"]);
        assert_eq!(task.name, "test amounts::parse::tests::adds");
        // In a nested module, async.
        let task = test_at(&path, LIB, 22, &dir.0).unwrap();
        assert!(task.args.contains(&"amounts::parse::tests::nested::deeper".to_string()), "{:?}", task.args);
        // After the nested module has closed.
        let task = test_at(&path, LIB, 28, &dir.0).unwrap();
        assert!(task.args.contains(&"amounts::parse::tests::after_nested".to_string()), "{:?}", task.args);
    }

    #[test]
    fn a_rust_line_outside_a_test_says_so() {
        let dir = TempDir::new("rust-none");
        let path = crate_with(&dir, "src/lib.rs", LIB);
        let err = test_at(&path, LIB, 2, &dir.0).unwrap_err();
        assert!(err.contains("isn't in a test"), "{err}");
        // A helper inside the test module isn't a test either.
        assert!(test_at(&path, LIB, 10, &dir.0).is_err());
    }

    #[test]
    fn rust_files_pick_their_test_binary() {
        let dir = TempDir::new("rust-targets");
        let text = "#[test]\nfn works() {}\n";
        let lib = crate_with(&dir, "src/lib.rs", text);
        assert_eq!(test_at(&lib, text, 2, &dir.0).unwrap().args[4..], ["--", "works", "--exact"]);
        let integration = dir.write("tests/cli.rs", text);
        assert_eq!(test_at(&integration, text, 2, &dir.0).unwrap().args[4..], ["--test", "cli", "--", "works", "--exact"]);
        let module = dir.write("src/a/mod.rs", text);
        assert!(test_at(&module, text, 1, &dir.0).unwrap().args.contains(&"a::works".to_string()));
        let bin = dir.write("src/bin/tool.rs", text);
        assert_eq!(test_at(&bin, text, 2, &dir.0).unwrap().args[4..], ["--bin", "tool", "--", "works", "--exact"]);
    }

    #[test]
    fn a_pytest_test_runs_by_its_node_id() {
        let dir = TempDir::new("python");
        let text = "import pytest\n\nclass TestLedger:\n    def setup_method(self):\n        pass\n\n    def test_totals(self):\n        x = 1\n\n        assert x == 1\n\ndef test_plain():\n    assert True\n\ndef helper():\n    pass\n";
        let path = dir.write("tests/test_ledger.py", text);
        let task = test_at(&path, text, 10, &dir.0).unwrap();
        assert_eq!((task.command.as_str(), task.args.clone()), ("pytest", vec!["tests/test_ledger.py::TestLedger::test_totals".to_string()]));
        let task = test_at(&path, text, 13, &dir.0).unwrap();
        assert_eq!(task.args, ["tests/test_ledger.py::test_plain"]);
        // On the class line: the whole class.
        let task = test_at(&path, text, 3, &dir.0).unwrap();
        assert_eq!(task.args, ["tests/test_ledger.py::TestLedger"]);
        assert!(test_at(&path, text, 16, &dir.0).is_err());
        assert!(test_at(&path, text, 4, &dir.0).is_err(), "setup_method isn't a test");
    }

    #[test]
    fn a_go_test_runs_by_its_name_in_its_package() {
        let dir = TempDir::new("go");
        let text = "package ledger\n\nimport \"testing\"\n\nfunc TestTotals(t *testing.T) {\n\tif 1 != 1 {\n\t\tt.Fatal(\"no\")\n\t}\n}\n\nfunc helper() {}\n";
        let path = dir.write("internal/ledger/ledger_test.go", text);
        let task = test_at(&path, text, 7, &dir.0).unwrap();
        assert_eq!(task.args, ["test", "-run", "^TestTotals$", "./internal/ledger"]);
        assert!(test_at(&path, text, 11, &dir.0).is_err());
    }

    #[test]
    fn other_files_say_they_cant_yet() {
        let err = test_at(Path::new("/p/a.test.ts"), "it('x', () => {})\n", 1, Path::new("/p")).unwrap_err();
        assert!(err.contains(".ts files"), "{err}");
    }
}
