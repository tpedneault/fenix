use std::path::PathBuf;

use crate::store::AgendaStore;

/// `dirs::config_dir()/fenix/agenda.json` -- same location convention
/// `fenix-config`'s own `Config::default_path` established for
/// `config.ini`. `None` only on a platform with no notion of a config
/// directory.
pub fn default_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("fenix").join("agenda.json"))
}

/// Loads the store from `path`. A missing or corrupt file loads as an
/// empty store rather than failing -- the agenda is a convenience, not
/// critical data that should refuse to open Fenix over, same posture
/// `fenix-config`'s `load_or_default` already has for `config.ini`.
pub fn load(path: &std::path::Path) -> AgendaStore {
    std::fs::read_to_string(path).ok().and_then(|contents| serde_json::from_str(&contents).ok()).unwrap_or_default()
}

pub fn save(path: &std::path::Path, store: &AgendaStore) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(store).map_err(std::io::Error::other)?;
    fenix_storage::write(path, &json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Priority;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_path(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("fenix-agenda-test-{name}-{}-{n}.json", std::process::id()))
    }

    #[test]
    fn loading_a_missing_file_yields_an_empty_store_not_an_error() {
        let store = load(&temp_path("missing"));
        assert!(store.tasks.is_empty());
        assert!(store.active_timer.is_none());
    }

    #[test]
    fn loading_a_corrupt_file_yields_an_empty_store_not_a_panic() {
        let path = temp_path("corrupt");
        std::fs::write(&path, b"not json").unwrap();
        let store = load(&path);
        assert!(store.tasks.is_empty());
    }

    #[test]
    fn a_store_round_trips_through_save_and_load() {
        let path = temp_path("round_trip");
        let mut store = AgendaStore::default();
        let a = store.create_task("Write the plan".to_string(), "details".to_string(), Priority::High, Some("Fenix".to_string()));
        let b = store.create_task("Review it".to_string(), "".to_string(), Priority::Low, None);
        store.add_dependency(b, a);
        store.add_subtask(a, "Draft outline".to_string());
        store.add_note(a, "started".to_string());
        store.log_manual_time(a, chrono::Duration::minutes(30));
        store.clock_in(b);

        save(&path, &store).unwrap();
        let reloaded = load(&path);

        assert_eq!(reloaded.tasks.len(), 2);
        assert_eq!(reloaded.task(a).unwrap().title, "Write the plan");
        assert_eq!(reloaded.task(b).unwrap().depends_on, vec![a]);
        assert_eq!(reloaded.task(a).unwrap().subtasks.len(), 1);
        assert_eq!(reloaded.task(a).unwrap().notes.len(), 1);
        assert_eq!(reloaded.task(a).unwrap().time_entries.len(), 1);
        assert_eq!(reloaded.active_timer.as_ref().map(|t| t.task_id), Some(b));
    }
}
