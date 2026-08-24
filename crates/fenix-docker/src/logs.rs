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
}
