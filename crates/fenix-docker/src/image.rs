use crate::engine;
use crate::process::run_ndjson;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub id: String,
    pub repository: String,
    pub tag: String,
    pub size: String,
}

/// Lists every local image -- mirrors `docker images` (or `podman
/// images`, see `engine::resolve`). Same never-fails posture as
/// `list_containers`.
pub fn list_images() -> Vec<Image> {
    run_ndjson(engine::resolve(), &["images", "--format", "{{json .}}"], parse_image)
}

fn parse_image(v: &serde_json::Value) -> Option<Image> {
    Some(Image {
        id: v.get("ID")?.as_str()?.to_string(),
        repository: v.get("Repository")?.as_str()?.to_string(),
        tag: v.get("Tag").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
        size: v.get("Size").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_shaped_docker_images_json_line() {
        let line = r#"{"ID":"sha256:deadbeef","Repository":"nginx","Tag":"latest","Digest":"<none>","CreatedSince":"2 weeks ago","CreatedAt":"2026-08-06 10:00:00 -0400 EDT","Size":"142MB"}"#;
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let img = parse_image(&v).unwrap();
        assert_eq!(img.id, "sha256:deadbeef");
        assert_eq!(img.repository, "nginx");
        assert_eq!(img.tag, "latest");
        assert_eq!(img.size, "142MB");
    }

    #[test]
    fn parse_image_rejects_an_object_missing_required_fields() {
        let v: serde_json::Value = serde_json::from_str(r#"{"Tag":"latest"}"#).unwrap();
        assert!(parse_image(&v).is_none());
    }

    #[test]
    fn list_images_never_fails_even_without_a_working_docker() {
        let _ = list_images();
    }
}
