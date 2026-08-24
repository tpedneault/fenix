use std::path::{Path, PathBuf};
use std::process::Command;

/// One `proc`/`namespace` definition found by shelling out to universal
/// ctags. Only the kinds `run` keeps (`p` procedure, `n` namespace) ever
/// produce a `TagEntry` -- ctags' default Tcl kinds already exclude
/// locals/parameters/`set` variables, which is the right scope for
/// "definitions" (paired with `tcl::KEYWORDS` for built-in-command
/// completion).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagEntry {
    pub name: String,
    pub file: PathBuf,
    pub line: usize,
}

/// Shells `ctags --fields=+n --languages={language} -R -f - {root}` and
/// parses the tab-separated vi-compatible output. Never fails -- a
/// missing `ctags` binary, a non-zero exit, or a directory with no
/// matches all just produce an empty `Vec`, the same "disclosed
/// degradation, not an error state" posture `fenix-project`'s own
/// `rg`/`git` shelling already takes for a missing external tool.
pub fn run(root: &Path, language: &str) -> Vec<TagEntry> {
    let output = Command::new("ctags")
        .arg("--fields=+n")
        .arg(format!("--languages={language}"))
        .arg("-R")
        .arg("-f")
        .arg("-")
        .arg(root)
        .output();

    match output {
        Ok(out) if out.status.success() => parse(&String::from_utf8_lossy(&out.stdout)),
        _ => Vec::new(),
    }
}

/// Parses plain vi-compatible ctags output:
/// `{name}\t{file}\t{ex-command};"\t{kind}\t{extra-fields...}`. Skips any
/// line starting with `!_` (Universal Ctags' pseudo-tag header lines,
/// e.g. `!_TAG_FILE_FORMAT`) -- present in some ctags configurations even
/// with plain `-f -` output -- and any line that doesn't split into at
/// least the four required fields (defensive: a malformed or unexpected
/// line is dropped, not a parse error for the whole file).
fn parse(text: &str) -> Vec<TagEntry> {
    let mut entries = Vec::new();
    for raw_line in text.lines() {
        if raw_line.starts_with("!_") {
            continue;
        }
        let fields: Vec<&str> = raw_line.split('\t').collect();
        if fields.len() < 4 {
            continue;
        }
        let name = fields[0];
        let file = fields[1];
        let kind = fields[3];
        if kind != "p" && kind != "n" {
            continue;
        }
        let line = fields[4..]
            .iter()
            .find_map(|f| f.strip_prefix("line:"))
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(0);
        entries.push(TagEntry { name: name.to_string(), file: PathBuf::from(file), line });
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fenix-completion-ctags-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn finds_procs_and_namespaces_in_a_real_tcl_file() {
        let dir = temp_dir("procs-and-namespaces");
        fs::write(
            dir.join("foo.tcl"),
            "namespace eval myns {\n    proc greet {name} {\n        puts \"hello $name\"\n    }\n}\n\nproc top_level_proc {} {\n    return 1\n}\n",
        )
        .unwrap();

        let mut entries = run(&dir, "Tcl");
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "greet");
        assert_eq!(entries[0].line, 2);
        assert_eq!(entries[1].name, "myns");
        assert_eq!(entries[1].line, 1);
        assert_eq!(entries[2].name, "top_level_proc");
        assert_eq!(entries[2].line, 7);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_with_no_definitions_produces_no_entries() {
        let dir = temp_dir("no-definitions");
        fs::write(dir.join("plain.tcl"), "puts hello\nif {1} { puts yes }\n").unwrap();

        let entries = run(&dir, "Tcl");
        assert!(entries.is_empty());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_nonexistent_root_never_panics_and_returns_empty() {
        let dir = std::env::temp_dir().join("fenix-completion-ctags-test-does-not-exist");
        let _ = fs::remove_dir_all(&dir);
        let entries = run(&dir, "Tcl");
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_skips_pseudo_tag_header_lines() {
        // Real pseudo-tag lines are short (3 fields) and would already be
        // dropped by the `fields.len() < 4` guard alone -- this line is
        // deliberately padded to 4+ fields so the test exercises the `!_`
        // prefix check specifically, not just the length guard.
        let text = "!_TAG_FILE_FORMAT\t2\t/comment/\textra\nfoo\tbar.tcl\t/^proc foo {} {$/;\"\tp\tline:1\n";
        let entries = parse(text);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "foo");
    }
}
