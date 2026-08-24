use std::path::Path;

use crate::process::run_action;

fn start_args(id: &str) -> Vec<String> {
    vec!["start".to_string(), id.to_string()]
}

fn stop_args(id: &str) -> Vec<String> {
    vec!["stop".to_string(), id.to_string()]
}

fn restart_args(id: &str) -> Vec<String> {
    vec!["restart".to_string(), id.to_string()]
}

fn remove_container_args(id: &str) -> Vec<String> {
    vec!["rm".to_string(), "-f".to_string(), id.to_string()]
}

fn remove_image_args(id: &str) -> Vec<String> {
    vec!["rmi".to_string(), id.to_string()]
}

fn run_image_args(image: &str) -> Vec<String> {
    vec!["run".to_string(), "-d".to_string(), image.to_string()]
}

fn build_args(context_dir: &Path, tag: Option<&str>) -> Vec<String> {
    let mut args = vec!["build".to_string()];
    if let Some(tag) = tag {
        args.push("-t".to_string());
        args.push(tag.to_string());
    }
    args.push(context_dir.to_string_lossy().into_owned());
    args
}

pub fn start_container(id: &str) -> Result<String, String> {
    run_action(&start_args(id))
}

pub fn stop_container(id: &str) -> Result<String, String> {
    run_action(&stop_args(id))
}

pub fn restart_container(id: &str) -> Result<String, String> {
    run_action(&restart_args(id))
}

pub fn remove_container(id: &str) -> Result<String, String> {
    run_action(&remove_container_args(id))
}

pub fn remove_image(id: &str) -> Result<String, String> {
    run_action(&remove_image_args(id))
}

/// Creates and starts a detached container from `image` -- the "run"
/// action on an image row in the panel.
pub fn run_image(image: &str) -> Result<String, String> {
    run_action(&run_image_args(image))
}

/// Builds `context_dir` (expected to contain a `Dockerfile`) via
/// `docker build`, tagging the result when `tag` is given.
pub fn build_image(context_dir: &Path, tag: Option<&str>) -> Result<String, String> {
    run_action(&build_args(context_dir, tag))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_args_targets_the_given_container() {
        assert_eq!(start_args("abc123"), vec!["start", "abc123"]);
    }

    #[test]
    fn stop_args_targets_the_given_container() {
        assert_eq!(stop_args("abc123"), vec!["stop", "abc123"]);
    }

    #[test]
    fn restart_args_targets_the_given_container() {
        assert_eq!(restart_args("abc123"), vec!["restart", "abc123"]);
    }

    #[test]
    fn remove_container_args_forces_removal() {
        assert_eq!(remove_container_args("abc123"), vec!["rm", "-f", "abc123"]);
    }

    #[test]
    fn remove_image_args_targets_the_given_image() {
        assert_eq!(remove_image_args("sha256:dead"), vec!["rmi", "sha256:dead"]);
    }

    #[test]
    fn run_image_args_runs_detached() {
        assert_eq!(run_image_args("nginx:latest"), vec!["run", "-d", "nginx:latest"]);
    }

    #[test]
    fn build_args_without_a_tag_just_passes_the_context_dir() {
        assert_eq!(build_args(Path::new("/proj"), None), vec!["build", "/proj"]);
    }

    #[test]
    fn build_args_with_a_tag_includes_dash_t() {
        assert_eq!(build_args(Path::new("/proj"), Some("myapp:latest")), vec!["build", "-t", "myapp:latest", "/proj"]);
    }

    #[test]
    fn actions_never_panic_even_without_a_working_docker() {
        let _ = start_container("nonexistent");
        let _ = stop_container("nonexistent");
        let _ = restart_container("nonexistent");
        let _ = remove_container("nonexistent");
        let _ = remove_image("nonexistent");
        let _ = run_image("nonexistent");
        let _ = build_image(Path::new("/nonexistent"), None);
    }
}
