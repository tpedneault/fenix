//! A formatter an extension gives (`formatter-set`): a program Fenix runs
//! with a buffer's text on its stdin, whose stdout is that text
//! formatted -- `SPC c F`, and saving with `editor.format_on_save`. It
//! runs while you wait, so it has a few seconds at most.

use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use fenix_project::tools::CommandSpec;

/// How long a formatter may take before it's stopped and the text kept.
pub const TIMEOUT: Duration = Duration::from_secs(3);

/// `spec` run on `text`, the file `file`'s, in the project at `root`
/// (`${file}` in its args is `file`): what it wrote, when it ended well
/// within `timeout` -- or, with `keep_output`, when it failed but
/// wrote something (ktlint); else why not.
pub fn run(spec: &CommandSpec, text: &str, file: &Path, root: &Path, timeout: Duration, keep_output: bool) -> Result<String, String> {
    let mut spec = spec.clone();
    let shown = file.to_string_lossy();
    spec.args = spec.args.iter().map(|a| a.replace("${file}", &shown)).collect();
    let name = Path::new(&spec.executable).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| spec.executable.clone());
    let mut command = spec.command(root).map_err(|e| format!("{name}: {e}"))?;
    command.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().map_err(|e| format!("{name}: {e}"))?;
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let input = text.to_string();
    // Written and read on threads of their own: a formatter may write
    // before it has read everything, and a pipe holds only so much.
    let writer = std::thread::spawn(move || {
        if let Some(mut stdin) = stdin {
            let _ = stdin.write_all(input.as_bytes());
        }
    });
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut out);
            }
            out
        })
    };
    let out = read(stdout.map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = read(stderr.map(|p| Box::new(p) as Box<dyn Read + Send>));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{name} took longer than {} s -- the text is as it was", timeout.as_secs()));
            }
            Err(e) => return Err(format!("{name}: {e}")),
        }
    };
    let _ = writer.join();
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    if !status.success() && !(keep_output && !out.is_empty()) {
        let said = String::from_utf8_lossy(&err);
        let first = said.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
        let code = status.code().map(|c| format!(" ({c})")).unwrap_or_default();
        return Err(format!("{name} failed{code}{}{first}", if first.is_empty() { "" } else { ": " }));
    }
    String::from_utf8(out).map_err(|_| format!("{name} wrote something that isn't text"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A program that reads its stdin to the end and writes it back
    /// sorted: `sort`, on every system.
    fn sort() -> CommandSpec {
        CommandSpec::new("sort".into(), Vec::new())
    }

    #[test]
    fn the_text_goes_in_and_what_it_writes_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        // Not too short: Windows' `sort` takes four bytes for UTF-16.
        let got = run(&sort(), "pear\napple\n", &dir.path().join("x.txt"), dir.path(), TIMEOUT, false).unwrap();
        assert_eq!(got.replace("\r\n", "\n"), "apple\npear\n");
    }

    #[test]
    fn a_formatter_that_fails_or_isnt_there_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let missing = CommandSpec::new("fenix-no-such-formatter".into(), Vec::new());
        assert!(run(&missing, "x", &dir.path().join("x.txt"), dir.path(), TIMEOUT, false).unwrap_err().starts_with("fenix-no-such-formatter: "));
        // `sort` of a file that isn't there fails, and says why.
        let failing = CommandSpec::new("sort".into(), vec!["${file}".into()]);
        let err = run(&failing, "x", &dir.path().join("gone.txt"), dir.path(), TIMEOUT, false).unwrap_err();
        assert!(err.starts_with("sort failed"), "{err}");
    }

    /// one that fails but wrote its text has it kept, when it's
    /// asked; one that wrote nothing still fails.
    #[test]
    fn what_a_failing_formatter_wrote_is_kept_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let (program, args) = if cfg!(windows) { ("cmd", vec!["/C", "echo formatted& exit 1"]) } else { ("sh", vec!["-c", "echo formatted; exit 1"]) };
        let failing = CommandSpec::new(program.into(), args.into_iter().map(String::from).collect());
        let file = dir.path().join("x.kt");
        assert!(run(&failing, "x", &file, dir.path(), TIMEOUT, false).unwrap_err().contains("failed (1)"));
        assert_eq!(run(&failing, "x", &file, dir.path(), TIMEOUT, true).unwrap().trim(), "formatted");
        let silent = CommandSpec::new("sort".into(), vec!["${file}".into()]);
        assert!(run(&silent, "x", &dir.path().join("gone.txt"), dir.path(), TIMEOUT, true).is_err(), "nothing written: a failure");
    }
}
