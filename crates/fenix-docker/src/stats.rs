use crate::engine;
use crate::process::run_ndjson;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerStat {
    pub container_id: String,
    /// Already human-formatted by `docker stats` itself (e.g. `"0.15%"`)
    /// -- kept as a string, not parsed into a float, since the panel
    /// only ever displays it, never computes with it.
    pub cpu_percent: String,
    /// Also already human-formatted (e.g. `"12MiB / 1.9GiB"`).
    pub mem_usage: String,
}

/// One-shot CPU/mem snapshot for every running container -- mirrors
/// `docker stats --no-stream` (or `podman stats --no-stream`, see
/// `engine::resolve`), not the continuously-streaming default (no
/// `--no-stream`), so this returns promptly instead of blocking
/// forever; call it on a timer for a "live" feel instead. Same
/// never-fails posture as every other `fenix-docker` listing function.
pub fn container_stats() -> Vec<ContainerStat> {
    run_ndjson(engine::resolve(), &["stats", "--no-stream", "--format", "{{json .}}"], parse_stat)
}

fn parse_stat(v: &serde_json::Value) -> Option<ContainerStat> {
    Some(ContainerStat {
        container_id: v.get("ID")?.as_str()?.to_string(),
        cpu_percent: v.get("CPUPerc").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
        mem_usage: v.get("MemUsage").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_shaped_docker_stats_json_line() {
        let line = r#"{"Container":"web","Name":"web","ID":"abc123","CPUPerc":"0.15%","MemUsage":"12MiB / 1.9GiB","MemPerc":"0.63%","NetIO":"1.2kB / 0B","BlockIO":"0B / 0B","PIDs":"3"}"#;
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let stat = parse_stat(&v).unwrap();
        assert_eq!(stat.container_id, "abc123");
        assert_eq!(stat.cpu_percent, "0.15%");
        assert_eq!(stat.mem_usage, "12MiB / 1.9GiB");
    }

    #[test]
    fn parse_stat_rejects_an_object_missing_required_fields() {
        let v: serde_json::Value = serde_json::from_str(r#"{"CPUPerc":"0.00%"}"#).unwrap();
        assert!(parse_stat(&v).is_none());
    }

    #[test]
    fn container_stats_never_fails_even_without_a_working_docker() {
        let _ = container_stats();
    }
}
