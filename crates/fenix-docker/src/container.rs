use crate::process::run_ndjson;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    /// Human status with duration/health, e.g. "Up 3 hours" or "Exited (0) 2 days ago".
    pub status: String,
    /// Docker's own coarse state, e.g. "running", "exited", "created".
    pub state: String,
}

/// Lists every container, running or not (`-a`) -- mirrors `docker ps -a`,
/// not just the running subset, since a Lazydocker-style panel needs to
/// show stopped containers too (that's the whole point of a `start`
/// action existing). Never fails -- a missing `docker` binary, an
/// unreachable daemon, or malformed output all yield an empty `Vec`, the
/// same posture `ctags::run`/`grep_project` already established for a
/// missing external tool.
pub fn list_containers() -> Vec<Container> {
    run_ndjson("docker", &["ps", "-a", "--format", "{{json .}}"], parse_container)
}

fn parse_container(v: &serde_json::Value) -> Option<Container> {
    Some(Container {
        id: v.get("ID")?.as_str()?.to_string(),
        name: v.get("Names")?.as_str()?.to_string(),
        image: v.get("Image")?.as_str()?.to_string(),
        status: v.get("Status").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
        state: v.get("State").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_shaped_docker_ps_json_line() {
        let line = r#"{"ID":"abc123","Image":"nginx:latest","Command":"\"nginx -g daemon off;\"","CreatedAt":"2026-08-20 10:00:00 -0400 EDT","RunningFor":"3 days ago","Ports":"0.0.0.0:80->80/tcp","State":"running","Status":"Up 3 days","Size":"0B","Names":"web","Labels":""}"#;
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let c = parse_container(&v).unwrap();
        assert_eq!(c.id, "abc123");
        assert_eq!(c.name, "web");
        assert_eq!(c.image, "nginx:latest");
        assert_eq!(c.status, "Up 3 days");
        assert_eq!(c.state, "running");
    }

    #[test]
    fn parse_container_rejects_an_object_missing_required_fields() {
        let v: serde_json::Value = serde_json::from_str(r#"{"Image":"nginx"}"#).unwrap();
        assert!(parse_container(&v).is_none());
    }

    #[test]
    fn list_containers_never_fails_even_without_a_working_docker() {
        // Whatever `docker` does on this machine (missing, daemon
        // unreachable, permission denied) -- this must not panic, and
        // must degrade to an empty list rather than propagating an error.
        let _ = list_containers();
    }
}
