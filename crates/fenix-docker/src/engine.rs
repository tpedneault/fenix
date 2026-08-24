use std::process::Command;
use std::sync::OnceLock;

/// Which container-engine binary every other module in this crate
/// shells out to -- resolved once per process and cached (probing spawns
/// a subprocess, not free to redo on every list/action call). Prefers
/// `docker` when it's actually runnable: this also transparently covers
/// Podman's own `podman-docker` compatibility package, which makes
/// `docker` itself resolve to `podman` under the hood -- indistinguishable
/// from here, and needing no special-casing since that shim's entire
/// purpose is CLI compatibility. Falls back to `podman` only when
/// `docker` isn't runnable at all but `podman` is (a plain Podman
/// install with no `docker` shim). Defaults to `"docker"` when neither
/// is found, preserving the pre-Podman-support behavior: every
/// `fenix-docker` call already degrades to an empty result/`Err` rather
/// than panicking when its binary is missing, so this default doesn't
/// change that -- it just keeps the same binary name in error messages.
pub(crate) fn resolve() -> &'static str {
    static RESOLVED: OnceLock<&'static str> = OnceLock::new();
    RESOLVED.get_or_init(|| resolve_from(probe("docker"), probe("podman")))
}

/// `name --version` succeeding is enough to know the binary is present
/// and runnable -- both Docker and Podman support it without needing a
/// reachable daemon, so this can't itself be fooled by an unreachable
/// daemon the way probing with `ps`/`images` could be.
fn probe(name: &str) -> bool {
    Command::new(name).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn resolve_from(docker_ok: bool, podman_ok: bool) -> &'static str {
    if docker_ok {
        "docker"
    } else if podman_ok {
        "podman"
    } else {
        "docker"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_from_prefers_docker_when_both_are_usable() {
        assert_eq!(resolve_from(true, true), "docker");
    }

    #[test]
    fn resolve_from_falls_back_to_podman_when_docker_is_not_usable() {
        assert_eq!(resolve_from(false, true), "podman");
    }

    #[test]
    fn resolve_from_defaults_to_docker_when_neither_is_usable() {
        assert_eq!(resolve_from(false, false), "docker");
    }

    #[test]
    fn probe_never_panics_for_a_nonexistent_binary() {
        assert!(!probe("fenix-test-nonexistent-binary-xyz"));
    }

    #[test]
    fn resolve_never_panics_and_returns_a_stable_value() {
        // Whatever's actually installed on this machine -- this just
        // confirms it doesn't panic and the cached value is consistent
        // across repeated calls within the same test process.
        assert_eq!(resolve(), resolve());
    }
}
