//! What's kept across a restart (§4): results, not tests, as the text
//! of `tests/<hash of the project's root>.json`. The model writes and
//! reads the text; the host names the file, writes it through
//! `fenix-storage` at a run's end and reads it when the project's first
//! provider starts.

use crate::limits::{self, KEPT_TEXT_BYTES, MESSAGE_BYTES, PATH_BYTES, RESULTS_FILE_BYTES};
use crate::model::{Model, ProviderId, TestKey, TestResult};
use crate::records::{Location, Outcome};
use serde::{Deserialize, Serialize};

/// 2 keys a result by its provider's extension too (Decisions, 18): a
/// file of 1, which didn't, is refused as unreadable, not misread, and
/// the next run's end writes a 2.
const VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    results: Vec<Kept>,
}

#[derive(Serialize, Deserialize)]
struct Kept {
    extension: String,
    provider: String,
    id: String,
    outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    duration_ms: Option<f64>,
    when_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at: Option<Location>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

/// Why a results file wasn't read. It's left as it was: the next run's
/// end writes a good one.
#[derive(Debug, PartialEq, Eq)]
pub enum ResultsFileError {
    /// Over 8 MiB: not Fenix's.
    TooLarge(usize),
    /// Not a results file, or a later Fenix's.
    Unreadable(String),
}

impl std::fmt::Display for ResultsFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResultsFileError::TooLarge(bytes) => write!(f, "the test results file is {bytes} bytes; at most 8 MiB is read"),
            ResultsFileError::Unreadable(why) => write!(f, "the test results file can't be read: {why}"),
        }
    }
}

impl std::error::Error for ResultsFileError {}

impl Model {
    /// The results file's text (§4): every result, a failure's text cut
    /// to its last 4 KiB, at most 8 MiB, newest first, the oldest left
    /// out past it. Results read at start for tests not found since are
    /// written again (Decisions, 18).
    pub fn results_file(&self) -> String {
        let mut kept: Vec<Kept> = Vec::new();
        for (_, node) in self.nodes() {
            if let Some(result) = &node.result {
                kept.push(Self::kept(&self.providers[node.provider as usize].id, &node.id, result));
            }
        }
        for (key, result) in &self.restored {
            kept.push(Self::kept(&key.provider, &key.id, result));
        }
        kept.sort_by(|a, b| b.when_ms.cmp(&a.when_ms).then_with(|| (&a.provider, &a.extension, &a.id).cmp(&(&b.provider, &b.extension, &b.id))));
        let head = format!("{{\"version\":{VERSION},\"results\":[");
        let mut out = head;
        let mut first = true;
        for k in kept {
            let Ok(item) = serde_json::to_string(&k) else { continue };
            if out.len() + item.len() + 3 > RESULTS_FILE_BYTES {
                break;
            }
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&item);
        }
        out.push_str("]}");
        out
    }

    fn kept(provider: &ProviderId, id: &str, result: &TestResult) -> Kept {
        Kept {
            extension: provider.extension.to_string(),
            provider: provider.provider.to_string(),
            id: id.to_string(),
            outcome: result.outcome,
            duration_ms: result.duration_ms.filter(|d| d.is_finite()),
            when_ms: result.when_ms,
            message: result.message.clone(),
            at: result.at.clone(),
            text: result.text.as_deref().map(|t| limits::cut_end(t, KEPT_TEXT_BYTES).to_string()),
        }
    }

    /// Reads a results file's text: each result goes to its test, shown
    /// stale until it runs again (§4), or waits for the test to be found.
    /// A test that has a result already keeps it. Its strings are held to
    /// the sizes a provider's are: the file is only Fenix's by its name.
    /// Returns how many results it read.
    pub fn restore_results(&mut self, text: &str) -> Result<usize, ResultsFileError> {
        if text.len() > RESULTS_FILE_BYTES {
            return Err(ResultsFileError::TooLarge(text.len()));
        }
        // Its version first: another version's results aren't read as
        // this one's, whatever their fields.
        #[derive(Deserialize)]
        struct Version {
            version: u32,
        }
        let version: Version = serde_json::from_str(text).map_err(|e| ResultsFileError::Unreadable(e.to_string()))?;
        if version.version != VERSION {
            return Err(ResultsFileError::Unreadable(format!("its version is {}", version.version)));
        }
        let file: File = serde_json::from_str(text).map_err(|e| ResultsFileError::Unreadable(e.to_string()))?;
        let mut read = 0;
        for k in file.results {
            let id = limits::cut_id(&k.id).into_owned();
            let result = TestResult {
                outcome: k.outcome,
                duration_ms: k.duration_ms.filter(|d| d.is_finite() && *d >= 0.0),
                when_ms: k.when_ms,
                message: k.message.map(|m| limits::cut_start(&m, MESSAGE_BYTES).to_string()),
                at: k.at.map(|at| Location { path: limits::cut_start(&at.path, PATH_BYTES).to_string(), ..at }),
                text: k.text.map(|t| limits::cut_end(&t, KEPT_TEXT_BYTES).to_string()),
                printed: String::new(),
                restored: true,
                seq: 0,
            };
            let key = TestKey { provider: ProviderId::new(k.extension, k.provider), id };
            match self.find(&key) {
                Some(node) => {
                    if self.raw(node).is_some_and(|n| n.result.is_none()) {
                        self.update(node, |n| n.result = Some(result));
                    }
                }
                None => {
                    self.restored.entry(key).or_insert(result);
                }
            }
            read += 1;
        }
        self.changed();
        Ok(read)
    }
}
