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

/// `Id`, not `ID` -- Podman's own `image` report struct (`cmd/podman/
/// images/list.go`, the one `--format '{{json .}}'` actually marshals)
/// embeds `entities.ImageSummary` untouched for the id field, whose own
/// `json:"Id"` tag differs from Docker CLI's `imageContext` formatter
/// struct (confirmed against podman's source -- not assumed). Docker's
/// own key is checked first since it's the common case; every image
/// line failed this `?` unconditionally on Podman before this, which is
/// why the Images pane came up empty there.
fn image_id(v: &serde_json::Value) -> Option<String> {
    v.get("ID").or_else(|| v.get("Id"))?.as_str().map(str::to_string)
}

/// Docker's `Size` is already a human string (`"142MB"`); Podman's own
/// `ImageSummary.Size` (same struct `image_id`'s doc comment describes)
/// is a raw byte count instead -- formatted here rather than left blank,
/// using decimal (1000-based) units to match Docker CLI's own `go-
/// units.HumanSize` convention, so a mixed Docker/Podman-shaped feed
/// reads consistently.
fn image_size(v: &serde_json::Value) -> String {
    match v.get("Size") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.as_u64().map(format_bytes).unwrap_or_default(),
        _ => String::new(),
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}B")
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}

fn parse_image(v: &serde_json::Value) -> Option<Image> {
    Some(Image {
        id: image_id(v)?,
        repository: v.get("Repository")?.as_str()?.to_string(),
        tag: v.get("Tag").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
        size: image_size(v),
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
    fn parses_a_real_shaped_podman_images_json_line() {
        // Podman's `{{json .}}` for `images` marshals `Id` (not `ID`)
        // and a raw byte-count `Size` (not a human string like Docker's)
        // -- confirmed against Podman's own source (`cmd/podman/images/
        // list.go`'s `image` struct wrapping `entities.ImageSummary`,
        // whose `ID` field carries a `json:"Id"` tag). Every image line
        // failed the old `v.get("ID")?` unconditionally on Podman, which
        // is why the Images pane came up empty there.
        let line = r#"{"Id":"sha256:deadbeef","Repository":"nginx","Tag":"latest","Size":142000000}"#;
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let img = parse_image(&v).unwrap();
        assert_eq!(img.id, "sha256:deadbeef");
        assert_eq!(img.repository, "nginx");
        assert_eq!(img.tag, "latest");
        assert_eq!(img.size, "142.0MB");
    }

    #[test]
    fn format_bytes_uses_decimal_units_like_docker_cli_does() {
        assert_eq!(format_bytes(0), "0B");
        assert_eq!(format_bytes(999), "999B");
        assert_eq!(format_bytes(1_000), "1.0kB");
        assert_eq!(format_bytes(142_000_000), "142.0MB");
        assert_eq!(format_bytes(3_500_000_000), "3.5GB");
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
