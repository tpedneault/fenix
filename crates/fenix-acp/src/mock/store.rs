//! Where the mock keeps its sessions: a folder of JSON files (`--store`),
//! so list, load and resume work across restarts as Claude's
//! `~/.claude/projects` does, or memory when no folder is given.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::config::Settings;
use super::lock;

/// One session as stored: what `session/list` shows and what
/// `session/load` replays.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(super) struct Record {
    pub session_id: String,
    pub cwd: PathBuf,
    pub title: Option<String>,
    pub updated_at: String,
    /// For ordering: `updatedAt` alone can tie within a millisecond.
    pub updated_nanos: u64,
    pub mode: String,
    pub model: String,
    pub effort: String,
    pub fast: bool,
    /// The `session/update`s a load replays, in order.
    pub history: Vec<Value>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost: f64,
}

impl Record {
    pub fn settings(&self) -> Settings {
        let default = Settings::default();
        let or = |value: &str, fallback: String| {
            if value.is_empty() {
                fallback
            } else {
                value.to_string()
            }
        };
        Settings {
            mode: or(&self.mode, default.mode),
            model: or(&self.model, default.model),
            effort: or(&self.effort, default.effort),
            fast: self.fast,
        }
    }

    pub fn set_settings(&mut self, settings: &Settings) {
        self.mode = settings.mode.clone();
        self.model = settings.model.clone();
        self.effort = settings.effort.clone();
        self.fast = settings.fast;
    }
}

pub(super) struct Store {
    dir: Option<PathBuf>,
    memory: Mutex<HashMap<String, Record>>,
}

impl Store {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Store {
            dir,
            memory: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, id: &str) -> Option<Record> {
        match &self.dir {
            Some(dir) => {
                let text = std::fs::read(file(dir, id)?).ok()?;
                serde_json::from_slice(&text).ok()
            }
            None => lock(&self.memory).get(id).cloned(),
        }
    }

    pub fn save(&self, record: &Record) {
        match &self.dir {
            Some(dir) => {
                let Some(path) = file(dir, &record.session_id) else {
                    return;
                };
                let Ok(text) = serde_json::to_vec_pretty(record) else {
                    return;
                };
                // Written aside and renamed, so a mock killed mid-write leaves
                // the previous file, not half of one.
                let staged = path.with_extension("json.tmp");
                let _ = std::fs::create_dir_all(dir);
                if std::fs::write(&staged, text).is_ok() {
                    let _ = std::fs::rename(&staged, &path);
                }
            }
            None => {
                lock(&self.memory).insert(record.session_id.clone(), record.clone());
            }
        }
    }

    pub fn delete(&self, id: &str) {
        match &self.dir {
            Some(dir) => {
                if let Some(path) = file(dir, id) {
                    let _ = std::fs::remove_file(path);
                }
            }
            None => {
                lock(&self.memory).remove(id);
            }
        }
    }

    /// Every stored session, newest first.
    pub fn all(&self) -> Vec<Record> {
        let mut records: Vec<Record> = match &self.dir {
            Some(dir) => std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
                .filter_map(|entry| serde_json::from_slice(&std::fs::read(entry.path()).ok()?).ok())
                .collect(),
            None => lock(&self.memory).values().cloned().collect(),
        };
        records.sort_by_key(|record| std::cmp::Reverse(record.updated_nanos));
        records
    }
}

/// The file for a session id, or none for an id that isn't a plain name
/// (it comes from the client, and must not reach outside the folder).
fn file(dir: &Path, id: &str) -> Option<PathBuf> {
    let plain = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    plain.then(|| dir.join(format!("{id}.json")))
}

pub(super) fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// ISO 8601 in UTC with milliseconds, e.g. `2026-10-30T12:34:56.789Z`.
pub(super) fn iso(nanos: u64) -> String {
    let millis = nanos / 1_000_000;
    let (secs, ms) = (millis / 1000, millis % 1000);
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let (h, m, s) = (rem / 3600, rem % 3600 / 60, rem % 60);
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{ms:03}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_dates() {
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        // 2026-10-30T12:00:00.250Z
        assert_eq!(iso(1_793_361_600_250_000_000), "2026-10-30T12:00:00.250Z");
        assert_eq!(iso(951_782_400_000_000_000), "2000-02-29T00:00:00.000Z");
    }
}
