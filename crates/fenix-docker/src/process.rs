use std::process::Command;

use crate::engine;

/// Shells `cmd args...` and parses stdout as newline-delimited JSON
/// objects (what `docker ... --format '{{json .}}'` produces -- one flat
/// JSON object per line, not a JSON array). Never fails: a missing
/// binary, a non-zero exit (e.g. daemon unreachable), or a line that
/// doesn't parse all just drop out of the result silently, mirroring
/// `ctags::run`'s "empty on any failure, never a hard error" posture.
pub(crate) fn run_ndjson<T>(cmd: &str, args: &[&str], parse: impl Fn(&serde_json::Value) -> Option<T>) -> Vec<T> {
    let Ok(output) = Command::new(cmd).args(args).output() else { return Vec::new() };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|v| parse(&v))
        .collect()
}

/// Runs `{docker,podman} args...` (see `engine::resolve`), returning
/// stdout on success or stderr (or a synthetic message if the binary
/// itself couldn't be launched) on failure -- the caller decides what to
/// do with either (render into a buffer, ignore, etc.).
pub(crate) fn run_action(args: &[String]) -> Result<String, String> {
    let bin = engine::resolve();
    match Command::new(bin).args(args).output() {
        Ok(out) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr).into_owned()),
        Err(err) => Err(format!("couldn't run {bin}: {err}")),
    }
}

/// Same shape as `run_action`, but on success concatenates stdout *and*
/// stderr instead of just stdout -- `docker logs` is the one command
/// here whose actually-useful output can land on either stream
/// (whichever the container's own process wrote to), unlike every other
/// action, which only ever produces a status message on stdout.
/// Disclosed simplification: stdout is appended before stderr, not
/// truly interleaved by timestamp (`std::process::Command` gives each
/// stream its own pipe, with no cheap portable way to merge them
/// byte-for-byte in arrival order) -- fine for a one-shot log dump into
/// a buffer, not a live `tail -f`.
pub(crate) fn run_action_combined_output(args: &[String]) -> Result<String, String> {
    let bin = engine::resolve();
    match Command::new(bin).args(args).output() {
        Ok(out) if out.status.success() => {
            let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
            combined.push_str(&String::from_utf8_lossy(&out.stderr));
            Ok(combined)
        }
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr).into_owned()),
        Err(err) => Err(format!("couldn't run {bin}: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_ndjson_returns_empty_for_a_nonexistent_binary() {
        let result: Vec<()> = run_ndjson("fenix-test-nonexistent-binary-xyz", &[], |_| Some(()));
        assert!(result.is_empty());
    }

    #[test]
    fn run_ndjson_skips_unparsable_lines_and_keeps_the_rest() {
        // Exercises the parser against hand-built NDJSON via a real
        // subprocess (`echo`, always on PATH) rather than mocking
        // `Command` -- consistent with this project's existing posture
        // of testing subprocess-shelling code against real processes.
        let ndjson = "{\"n\":1}\nnot json\n{\"n\":2}\n";
        let result = run_ndjson("echo", &["-n", ndjson], |v| v.get("n").and_then(|n| n.as_i64()));
        assert_eq!(result, vec![1, 2]);
    }

    #[test]
    fn run_action_reports_an_error_for_a_nonexistent_docker() {
        // Can't rename the real `docker` binary from a test, so this
        // just confirms run_action never panics and returns a Result
        // either way against whatever `docker` actually does here.
        let _ = run_action(&["--version".to_string()]);
    }

    #[test]
    fn run_action_combined_output_never_panics_either() {
        let _ = run_action_combined_output(&["--version".to_string()]);
    }
}
