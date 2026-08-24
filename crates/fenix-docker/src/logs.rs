use std::process::{Child, Command, Stdio};

use crate::engine;
use crate::process::run_action_combined_output;

fn logs_args(id: &str, tail: usize) -> Vec<String> {
    vec!["logs".to_string(), "--tail".to_string(), tail.to_string(), id.to_string()]
}

/// The last `tail` lines of `id`'s combined stdout/stderr log output --
/// `SPC d d`'s `l` action. Never panics; a missing `docker`, a stopped
/// container with no logs, or an unreachable daemon all just come back
/// as an `Err` (or an `Ok` with empty text) for the caller to display.
pub fn container_logs(id: &str, tail: usize) -> Result<String, String> {
    run_action_combined_output(&logs_args(id, tail))
}

/// Spawns `{docker,podman} logs -f --tail N <id>` (see `engine::
/// resolve`) as a genuinely long-lived child process (stdout/stderr
/// piped, both inherited into the same pipe reasoning `container_logs`
/// already documents) for live tailing -- unlike `container_logs`'s
/// one-shot snapshot, this process keeps running (streaming new output
/// as the container produces it) until the caller kills it. Returns
/// `None` (never panics) if the binary itself can't be launched; the
/// caller owns reading `child.stdout` on its own thread and is
/// responsible for eventually killing the child (this crate stays
/// GUI/event-loop-agnostic, so it doesn't manage that lifecycle itself
/// -- see `fenix-gui`'s `DockerLogFollower`).
pub fn spawn_log_follower(id: &str, tail: usize) -> Option<Child> {
    Command::new(engine::resolve())
        .args(["logs", "-f", "--tail", &tail.to_string(), id])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_args_requests_the_given_tail_and_container() {
        assert_eq!(logs_args("abc123", 200), vec!["logs", "--tail", "200", "abc123"]);
    }

    #[test]
    fn container_logs_never_panics_even_without_a_working_docker() {
        let _ = container_logs("nonexistent", 200);
    }

    #[test]
    fn spawn_log_follower_never_panics_even_without_a_working_docker() {
        // Whatever `engine::resolve()` picks is on `PATH` in this
        // environment, so this actually spawns a real process -- it
        // just can't reach a daemon. Kill it immediately so the test
        // doesn't leave a lingering `logs -f` child behind.
        if let Some(mut child) = spawn_log_follower("nonexistent", 10) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
