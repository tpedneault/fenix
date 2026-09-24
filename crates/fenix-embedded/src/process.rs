//! Running a toolchain query to completion and reading what it printed.

use std::path::Path;

/// Runs `program args` in `cwd` and returns its stdout. A non-zero exit
/// is an error carrying whatever the tool said, since that's the most
/// useful thing to show. No console window flashes up on Windows.
pub fn output(program: &Path, args: &[&str], cwd: Option<&Path>) -> Result<String, String> {
    let mut command = std::process::Command::new(program);
    command.args(args).stdin(std::process::Stdio::null());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let out = command.output().map_err(|err| format!("couldn't run {}: {err}", program.display()))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        return Ok(stdout);
    }
    // arduino-cli's `--format json` puts its error in stdout as
    // `{"error": "..."}`; everything else says it on stderr.
    let json_error = serde_json::from_str::<serde_json::Value>(&stdout)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string));
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(json_error.unwrap_or(if stderr.is_empty() { stdout.trim().to_string() } else { stderr }))
}
