use crate::engine;
use crate::process::run_ndjson;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
}

/// Lists every network -- mirrors `docker network ls` (or `podman
/// network ls`, see `engine::resolve`). Same never-fails posture as
/// `list_containers`/`list_images`/`list_volumes`.
pub fn list_networks() -> Vec<Network> {
    run_ndjson(engine::resolve(), &["network", "ls", "--format", "{{json .}}"], parse_network)
}

/// Docker's `network ls --format '{{json .}}'` is confirmed to use `ID`
/// (docs.docker.com/reference/cli/docker/network/ls). There's no
/// confirmed report of Podman shipping `Id` here the way `images` does
/// (unlike `image::image_id`'s doc comment, this isn't evidenced) -- but
/// falling back defensively costs nothing and matches this crate's
/// established posture (see `container::parse_container`'s own `Id`/`ID`
/// fallback comment) given the two commands' JSON has a documented
/// history of drifting independently between Podman versions.
fn parse_network(v: &serde_json::Value) -> Option<Network> {
    Some(Network {
        id: v.get("ID").or_else(|| v.get("Id"))?.as_str()?.to_string(),
        name: v.get("Name")?.as_str()?.to_string(),
        driver: v.get("Driver").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
        // Podman has no `Scope` concept for networks at all (no swarm
        // mode) -- naturally empty-strings via `.unwrap_or_default()`,
        // same as every other optional field here and in `volume.rs`.
        scope: v.get("Scope").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_shaped_docker_network_ls_json_line() {
        let line = r#"{"CreatedAt":"2021-03-09 21:41:29.798999529 +0000 UTC","Driver":"bridge","ID":"f33ba176dd8e","IPv6":"false","Internal":"false","Labels":"","Name":"bridge","Scope":"local"}"#;
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let n = parse_network(&v).unwrap();
        assert_eq!(n.id, "f33ba176dd8e");
        assert_eq!(n.name, "bridge");
        assert_eq!(n.driver, "bridge");
        assert_eq!(n.scope, "local");
    }

    #[test]
    fn parses_a_podman_network_ls_json_line_with_id_instead_of_uppercase_id() {
        // Same defensive fallback `container::parse_container` already
        // has for `ps` -- see this module's `parse_network` doc comment.
        let line = r#"{"Id":"abc123","Name":"podman","Driver":"bridge"}"#;
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let n = parse_network(&v).unwrap();
        assert_eq!(n.id, "abc123");
        assert_eq!(n.name, "podman");
        // Podman has no `Scope` key at all -- degrades to empty, not a
        // rejected row.
        assert_eq!(n.scope, "");
    }

    #[test]
    fn parse_network_rejects_an_object_missing_required_fields() {
        let v: serde_json::Value = serde_json::from_str(r#"{"Driver":"bridge"}"#).unwrap();
        assert!(parse_network(&v).is_none());
    }

    #[test]
    fn list_networks_never_fails_even_without_a_working_docker() {
        let _ = list_networks();
    }
}
