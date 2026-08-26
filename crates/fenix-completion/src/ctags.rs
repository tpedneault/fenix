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
    /// The fully-qualified name, e.g. `myns::subns::greet` for a `proc
    /// greet` nested inside `namespace eval myns { namespace eval subns
    /// {...} }`, or plain `global_proc` for one defined at the top
    /// level -- never a leading `::`, even though Tcl's own fully-
    /// qualified names always have one (`::myns::subns::greet`), since
    /// completions should exclude the global-namespace prefix. Built
    /// from ctags' own `namespace:` extra field (confirmed present by
    /// default with just `--fields=+n`, already the flag `run` passes --
    /// no extra flag needed) plus the tag's own bare name; a tag with no
    /// `namespace:` field is already at the top level.
    pub name: String,
    pub file: PathBuf,
    pub line: usize,
}

/// Strips a Windows "verbatim"/extended-length path prefix (`\\?\`, or
/// its UNC-path sibling `\\?\UNC\`) if present -- `std::fs::canonicalize`
/// unconditionally adds this on Windows, so `root` can arrive already
/// in this form however Fenix got handed the path (`fenix-project::
/// find_project_root` itself doesn't canonicalize, but whatever the
/// original file path came from further upstream might have). Confirmed
/// empirically against a real Universal Ctags install: the *identical*
/// directory, scanned once with a plain path and once with its `\\?\`-
/// prefixed form, produces every real tag line vs. none at all -- no
/// error, no warning, just silently empty output, which is why this
/// needed to be tracked down as a real bug rather than assumed to be
/// cosmetic. A no-op on every other platform (the prefix can't occur
/// there) and for a `root` that was never canonicalized to begin with.
fn windows_friendly_path(root: &Path) -> PathBuf {
    let s = root.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        root.to_path_buf()
    }
}

/// Shells `ctags --fields=+n --languages={language} -R -f - {root}` and
/// parses the tab-separated vi-compatible output. Never fails outright
/// -- a missing `ctags` binary, a non-zero exit, or a directory with no
/// matches all just produce an empty `Vec`, the same "disclosed
/// degradation, not an error state" posture `fenix-project`'s own
/// `rg`/`git` shelling already takes for a missing external tool -- but
/// unlike those, every non-success path here is loud about *why*
/// (`eprintln!`, this crate's own established convention elsewhere,
/// e.g. `fenix_format`): a bare-not-found binary, a non-zero exit (with
/// its captured stderr -- the single most useful signal for something
/// like "this ctags build has no Tcl parser" or a rejected flag), and
/// any other spawn failure are each reported distinctly, since silently
/// returning an empty `Vec` gives a caller no way to tell "genuinely no
/// definitions found" apart from "ctags never actually ran."
pub fn run(root: &Path, language: &str) -> Vec<TagEntry> {
    let mut cmd = Command::new("ctags");
    cmd.arg("--fields=+n").arg(format!("--languages={language}"));
    // Universal Ctags' own default file mapping for Tcl is `*.tcl *.tk
    // *.wish *.exp` -- notably missing `.tm` (Tcl Modules, a common
    // real-world packaging convention: a namespace's procs defined at
    // the top level with fully-qualified names, e.g. `proc ns::sns::
    // name {...}`, in a file literally named after that path). `fenix-
    // syntax::detect_language` already treats `.tm` as Tcl for editing/
    // highlighting purposes -- without this, a project built entirely
    // out of `.tm` files gets silently skipped here: `ctags` exits 0
    // having genuinely found nothing to parse, not failed, so nothing
    // else in `run` (the "0 parsed but real tag lines came back" check
    // included) has anything to flag. Must come before the positional
    // `root` path below -- confirmed empirically that `ctags` silently
    // ignores an option placed after it instead of erroring.
    if language == "Tcl" {
        cmd.arg("--langmap=Tcl:+.tm");
    }
    cmd.arg("-R").arg("-f").arg("-").arg(windows_friendly_path(root));
    // Windows-only: without this, every shell-out here (and there are a
    // lot -- once per `SPC c r`, once per project root change) briefly
    // flashes a console window, since `fenix-gui` itself has no console
    // of its own to inherit. Purely cosmetic, no bearing on *why* a run
    // fails, but a real, easy robustness win specifically on the
    // platform this was reported on.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    match cmd.output() {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let entries = parse(&stdout);
            // A successful run that produced real tag lines but zero
            // *parsed* entries is a different, more actionable signal
            // than "this directory genuinely has no procs/namespaces" --
            // most likely an unexpected ctags output shape (a different
            // ctags implementation/version than this parser was written
            // against). Gated on real content existing at all, so an
            // honestly-empty project stays silent here.
            if entries.is_empty() {
                let other_lines = stdout.lines().filter(|l| !l.trim().is_empty() && !l.starts_with("!_")).count();
                if other_lines > 0 {
                    eprintln!(
                        "fenix: ctags scanned {} for {language} and produced {other_lines} tag line(s), but none parsed \
                         as a proc/namespace Fenix recognizes -- possibly a ctags output-format this parser doesn't \
                         expect (this is written against Universal Ctags' plain vi-compatible `-f -` output)",
                        root.display()
                    );
                }
            }
            entries
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stderr = stderr.trim();
            eprintln!(
                "fenix: ctags exited with {} while scanning {} for {language} definitions{}",
                out.status,
                root.display(),
                if stderr.is_empty() { String::new() } else { format!(" -- {stderr}") },
            );
            Vec::new()
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "fenix: ctags not found on PATH -- Tcl completion and symbol lookup (`SPC c s`) need Universal Ctags \
                 installed and reachable as `ctags` (`ctags.exe` on Windows resolves the same way). If you just \
                 installed it, restart your terminal/shell so the updated PATH is picked up."
            );
            Vec::new()
        }
        Err(err) => {
            eprintln!("fenix: couldn't run ctags while scanning {} for {language} definitions: {err}", root.display());
            Vec::new()
        }
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
        let namespace = fields[4..].iter().find_map(|f| f.strip_prefix("namespace:"));
        let qualified_name = match namespace {
            Some(ns) if !ns.is_empty() => format!("{}::{name}", ns.trim_start_matches("::")),
            _ => name.to_string(),
        };
        entries.push(TagEntry { name: qualified_name, file: PathBuf::from(file), line });
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
    fn windows_friendly_path_strips_the_verbatim_prefix() {
        assert_eq!(windows_friendly_path(Path::new(r"\\?\C:\Users\thoma\project")), Path::new(r"C:\Users\thoma\project"));
    }

    #[test]
    fn windows_friendly_path_strips_the_verbatim_unc_prefix() {
        assert_eq!(windows_friendly_path(Path::new(r"\\?\UNC\server\share\project")), Path::new(r"\\server\share\project"));
    }

    #[test]
    fn windows_friendly_path_leaves_a_plain_path_untouched() {
        let plain = Path::new(r"C:\Users\thoma\project");
        assert_eq!(windows_friendly_path(plain), plain);
    }

    #[cfg(windows)]
    #[test]
    fn finds_definitions_when_the_root_is_a_canonicalized_verbatim_path() {
        // The actual reported bug: `std::fs::canonicalize` always
        // returns a `\\?\`-prefixed path on Windows, and Universal
        // Ctags silently scans nothing when handed one directly.
        let dir = temp_dir("verbatim-root");
        fs::write(dir.join("foo.tcl"), "proc top_level_proc {} {\n    return 1\n}\n").unwrap();
        let canonical = fs::canonicalize(&dir).unwrap();
        assert!(canonical.to_string_lossy().starts_with(r"\\?\"), "expected canonicalize to add the verbatim prefix");

        let entries = run(&canonical, "Tcl");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "top_level_proc");

        fs::remove_dir_all(&dir).ok();
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
        assert_eq!(entries[0].name, "myns"); // top-level namespace, no qualifier
        assert_eq!(entries[0].line, 1);
        assert_eq!(entries[1].name, "myns::greet"); // qualified, no leading "::"
        assert_eq!(entries[1].line, 2);
        assert_eq!(entries[2].name, "top_level_proc");
        assert_eq!(entries[2].line, 7);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn nested_namespaces_produce_a_fully_qualified_name_without_a_leading_double_colon() {
        let dir = temp_dir("nested-namespaces");
        fs::write(
            dir.join("foo.tcl"),
            "namespace eval outer {\n    namespace eval inner {\n        proc deep {} {\n            return 1\n        }\n    }\n}\n",
        )
        .unwrap();

        let entries = run(&dir, "Tcl");
        let deep = entries.iter().find(|e| e.name.ends_with("deep")).expect("expected a 'deep' entry");
        assert_eq!(deep.name, "outer::inner::deep");
        assert!(!deep.name.starts_with("::"));

        let inner = entries.iter().find(|e| e.name.ends_with("inner")).expect("expected an 'inner' entry");
        assert_eq!(inner.name, "outer::inner");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fully_qualified_proc_names_are_found_even_with_an_empty_namespace_eval() {
        // A common real-world Tcl idiom: `namespace eval ns {}` just to
        // declare the namespace exists, then every proc defined at the
        // top level using its own fully-qualified `ns::sns::name` --
        // never nested inside the `namespace eval` block itself.
        let dir = temp_dir("empty-namespace-eval");
        fs::write(
            dir.join("foo.tcl"),
            "namespace eval ns {}\n\nproc ns::sns::proc_name {arg1} {\n    return $arg1\n}\n\nproc ns::other_proc {} {\n    return 1\n}\n",
        )
        .unwrap();

        let mut entries = run(&dir, "Tcl");
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(entries.len(), 3, "expected ns, ns::other_proc, ns::sns::proc_name -- got {entries:?}");
        assert_eq!(entries[0].name, "ns");
        assert_eq!(entries[1].name, "ns::other_proc");
        assert_eq!(entries[2].name, "ns::sns::proc_name");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finds_definitions_in_a_tm_file_not_just_tcl() {
        // Universal Ctags' own default Tcl file mapping doesn't include
        // `.tm` (Tcl Modules) -- `fenix-syntax::detect_language` treats
        // it as Tcl for editing, so `run` needs to as well, or a
        // project built out of `.tm` files reports zero definitions
        // with no error (a real, reported bug -- see `--langmap` above).
        let dir = temp_dir("tm-extension");
        fs::write(dir.join("foo.tm"), "proc ns::sns::proc_name {arg1} {\n    return $arg1\n}\n").unwrap();

        let entries = run(&dir, "Tcl");
        assert_eq!(entries.len(), 1, "expected ns::sns::proc_name to be found in a .tm file -- got {entries:?}");
        assert_eq!(entries[0].name, "ns::sns::proc_name");

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
