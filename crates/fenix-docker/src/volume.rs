use crate::engine;
use crate::process::run_ndjson;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
}

/// Lists every local volume -- mirrors `docker volume ls` (or `podman
/// volume ls`, see `engine::resolve`). Same never-fails posture as
/// `list_containers`/`list_images`.
pub fn list_volumes() -> Vec<Volume> {
    run_ndjson(engine::resolve(), &["volume", "ls", "--format", "{{json .}}"], parse_volume)
}

fn parse_volume(v: &serde_json::Value) -> Option<Volume> {
    Some(Volume {
        name: v.get("Name")?.as_str()?.to_string(),
        driver: v.get("Driver").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
        mountpoint: v.get("Mountpoint").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_shaped_docker_volume_ls_json_line() {
        let line = r#"{"Name":"myvol","Driver":"local","Scope":"local","Mountpoint":"/var/lib/docker/volumes/myvol/_data","Labels":"","Links":"0","Size":"N/A"}"#;
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let vol = parse_volume(&v).unwrap();
        assert_eq!(vol.name, "myvol");
        assert_eq!(vol.driver, "local");
        assert_eq!(vol.mountpoint, "/var/lib/docker/volumes/myvol/_data");
    }

    #[test]
    fn parse_volume_rejects_an_object_missing_required_fields() {
        let v: serde_json::Value = serde_json::from_str(r#"{"Driver":"local"}"#).unwrap();
        assert!(parse_volume(&v).is_none());
    }

    #[test]
    fn list_volumes_never_fails_even_without_a_working_docker() {
        let _ = list_volumes();
    }
}
