//! Whether a newer Fenix is out: GitHub's latest release of
//! `tpedneault/fenix`, read at most once a day when `updates.check` is
//! on, or when asked. Nothing is downloaded or installed -- Home says
//! there's a newer one, and where.
//!
//! When it last looked, and what it found, is kept in a small file of
//! Fenix's state (`updates.json`), so a restart doesn't look again.
//!
//! The channel (`updates.channel`) says which builds are told about.
//! `stable`, the default: releases alone. `nightly`: the rolling
//! `nightly` release too, built from `master` when it changed and stamped
//! `1.6.0-nightly.20261112`, which reads the release's own record of its
//! version and commit. A release newer than the nightly running is still
//! offered; going back to `stable` goes back to releases alone at once,
//! without looking again.

use std::path::Path;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::registry::Fetch;

/// Where the latest release is read.
pub const LATEST_URL: &str = "https://api.github.com/repos/tpedneault/fenix/releases/latest";

/// Where the rolling nightly release is read (`updates.channel =
/// "nightly"`).
pub const NIGHTLY_URL: &str = "https://api.github.com/repos/tpedneault/fenix/releases/tags/nightly";

/// Where people are sent to get one.
pub const RELEASES_PAGE: &str = "https://github.com/tpedneault/fenix/releases";

/// Where people are sent to get the nightly.
pub const NIGHTLY_PAGE: &str = "https://github.com/tpedneault/fenix/releases/tag/nightly";

/// The line in a nightly release's notes that records what it is, which
/// the nightly workflow writes: `<!-- fenix-nightly version=... commit=...
/// -->` (a comment, so GitHub doesn't show it).
pub const NIGHTLY_MARKER: &str = "<!-- fenix-nightly ";

/// How often the daily check looks.
pub const EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// The most of GitHub's answer that's read.
const MAX_ANSWER: u64 = 512 * 1024;

/// Which builds the update check tells you about (`updates.channel`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Channel {
    /// Releases alone.
    #[default]
    Stable,
    /// The nightly too, and releases newer than it.
    Nightly,
}

impl Channel {
    /// The setting's value; anything but `nightly` is `stable`.
    pub fn from_setting(value: Option<&str>) -> Channel {
        match value.map(str::trim) {
            Some(v) if v.eq_ignore_ascii_case("nightly") => Channel::Nightly,
            _ => Channel::Stable,
        }
    }
}

/// What was found when it last looked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    /// When, in seconds since 1970.
    #[serde(default)]
    pub checked: u64,
    /// The newest release then, as GitHub tags it (`v1.6.0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
    /// The nightly then, when the channel was `nightly`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nightly: Option<Nightly>,
}

/// The rolling nightly release: its version, and the commit it was built
/// from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Nightly {
    /// `1.6.0-nightly.20261112`.
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

/// A newer Fenix than the one running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Newer {
    /// A release (`1.6.0`).
    Release(semver::Version),
    /// The nightly, with the commit it was built from.
    Nightly { version: semver::Version, commit: Option<String> },
}

impl Newer {
    pub fn version(&self) -> &semver::Version {
        match self {
            Newer::Release(v) | Newer::Nightly { version: v, .. } => v,
        }
    }
}

impl Seen {
    /// `updates.json` in the state folder; none (never looked) when it
    /// isn't there or doesn't read.
    pub fn read(path: &Path) -> Seen {
        std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    pub fn write(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        fenix_storage::write(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Whether the daily check is due at `now`: never looked, a day
    /// since, or a clock that went back.
    pub fn due(&self, now: SystemTime) -> bool {
        let now = now.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        self.checked == 0 || now < self.checked || now - self.checked >= EVERY.as_secs()
    }

    /// When the daily check is next due.
    pub fn next(&self, now: SystemTime) -> SystemTime {
        if self.due(now) {
            return now;
        }
        SystemTime::UNIX_EPOCH + Duration::from_secs(self.checked) + EVERY
    }

    /// The newer Fenix than `running` it found, if any: a newer release,
    /// or on the `nightly` channel a newer nightly -- whichever is newer
    /// when both are. A nightly built from `running_commit` is the one
    /// running, whatever its version says.
    pub fn newer_than(&self, running: &semver::Version, running_commit: Option<&str>, channel: Channel) -> Option<Newer> {
        let release = self.latest.as_deref().and_then(release_version).filter(|v| v > running).map(Newer::Release);
        let nightly = match channel {
            Channel::Stable => None,
            Channel::Nightly => self.nightly.as_ref().and_then(|n| {
                let version = nightly_version(&n.version)?;
                let same_commit = matches!((running_commit, n.commit.as_deref()), (Some(a), Some(b)) if a.eq_ignore_ascii_case(b));
                (version > *running && !same_commit).then(|| Newer::Nightly { version, commit: n.commit.clone() })
            }),
        };
        release.into_iter().chain(nightly).max_by(|a, b| a.version().cmp(b.version()))
    }
}

/// A release's version from its tag: `v1.6.0` or `1.6.0`. A pre-release
/// (`v1.6.0-rc.1`) isn't one anyone's told about.
pub fn release_version(tag: &str) -> Option<semver::Version> {
    let v = semver::Version::parse(tag.trim().trim_start_matches('v')).ok()?;
    v.pre.is_empty().then_some(v)
}

/// A nightly's version: `1.6.0-nightly.20261112`, nothing else.
pub fn nightly_version(text: &str) -> Option<semver::Version> {
    let v = semver::Version::parse(text.trim().trim_start_matches('v')).ok()?;
    v.pre.as_str().starts_with("nightly.").then_some(v)
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// The latest release's tag, as GitHub says.
pub fn latest(fetch: &dyn Fetch) -> Result<String, String> {
    let bytes = fetch.get(LATEST_URL, MAX_ANSWER)?;
    let release: Release = serde_json::from_slice(&bytes).map_err(|e| format!("GitHub's answer didn't read: {e}"))?;
    if release.draft || release.prerelease || release_version(&release.tag_name).is_none() {
        return Err(format!("the latest release ({}) isn't one to offer", release.tag_name));
    }
    Ok(release.tag_name)
}

/// The rolling nightly release, from the record its notes carry (or, short
/// of one, its title, `Fenix 1.6.0-nightly.20261112`).
pub fn nightly(fetch: &dyn Fetch) -> Result<Nightly, String> {
    let bytes = fetch.get(NIGHTLY_URL, MAX_ANSWER)?;
    let release: Release = serde_json::from_slice(&bytes).map_err(|e| format!("GitHub's answer didn't read: {e}"))?;
    if release.draft || release.tag_name != "nightly" {
        return Err(format!("the nightly release ({}) isn't one to offer", release.tag_name));
    }
    let (mut version, mut commit) = (None, None);
    if let Some(record) = release.body.as_deref().and_then(|b| b.lines().find_map(|l| l.trim().strip_prefix(NIGHTLY_MARKER))) {
        for field in record.trim_end_matches("-->").split_whitespace() {
            match field.split_once('=') {
                Some(("version", v)) => version = Some(v.to_string()),
                Some(("commit", c)) if c.len() >= 7 && c.chars().all(|c| c.is_ascii_hexdigit()) => commit = Some(c.to_ascii_lowercase()),
                _ => {}
            }
        }
    }
    let version = version
        .filter(|v| nightly_version(v).is_some())
        .or_else(|| release.name.as_deref().and_then(|n| n.split_whitespace().find(|w| nightly_version(w).is_some())).map(str::to_string))
        .ok_or("the nightly release doesn't say its version")?;
    Ok(Nightly { version, commit })
}

/// Looks now: what was found, as of `now`. On the `nightly` channel the
/// nightly too; either answer alone will do.
pub fn check(fetch: &dyn Fetch, now: SystemTime, channel: Channel) -> Result<Seen, String> {
    let checked = now.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    match channel {
        Channel::Stable => Ok(Seen { checked, latest: Some(latest(fetch)?), nightly: None }),
        Channel::Nightly => match (latest(fetch), nightly(fetch)) {
            (Err(a), Err(b)) => Err(format!("{a}; {b}")),
            (latest, nightly) => Ok(Seen { checked, latest: latest.ok(), nightly: nightly.ok() }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GitHub, answering each URL from a list; any other is a 404.
    struct Answers(Vec<(&'static str, &'static str)>);
    impl Fetch for Answers {
        fn get(&self, url: &str, _limit: u64) -> Result<Vec<u8>, String> {
            self.0.iter().find(|(u, _)| *u == url).map(|(_, a)| a.as_bytes().to_vec()).ok_or_else(|| format!("{url}: status code 404"))
        }
    }

    fn latest_is(answer: &'static str) -> Answers {
        Answers(vec![(LATEST_URL, answer)])
    }

    fn v(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    const NIGHTLY: &str = r#"{"tag_name": "nightly", "name": "Fenix 1.6.0-nightly.20261112", "prerelease": true,
        "body": "Built from abc on 2026-11-12.\n\n<!-- fenix-nightly version=1.6.0-nightly.20261112 commit=0123456789ABCDEF0123456789abcdef01234567 -->\n"}"#;

    #[test]
    fn a_release_s_tag_is_its_version_and_a_pre_release_is_none() {
        assert_eq!(release_version("v1.6.0"), Some(v("1.6.0")));
        assert_eq!(release_version("1.6.0"), Some(v("1.6.0")));
        assert_eq!(release_version("v1.6.0-rc.1"), None);
        assert_eq!(release_version("nightly"), None);
        assert_eq!(nightly_version("1.6.0-nightly.20261112"), Some(v("1.6.0-nightly.20261112")));
        assert_eq!(nightly_version("1.6.0-rc.1"), None);
        assert_eq!(nightly_version("1.6.0"), None);
    }

    /// What scripts/nightly.sh stamps sorts after the release before it,
    /// after earlier nightlies, and before its own release.
    #[test]
    fn nightly_versions_sort_between_releases() {
        let order = ["1.5.2", "1.6.0-nightly.20261111", "1.6.0-nightly.20261112", "1.6.0-nightly.20261112.2", "1.6.0-nightly.20261112.10", "1.6.0-nightly.20261113", "1.6.0", "1.6.1", "1.7.0-nightly.20261201"];
        for pair in order.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        }
    }

    #[test]
    fn the_channel_is_stable_unless_it_says_nightly() {
        assert_eq!(Channel::from_setting(None), Channel::Stable);
        assert_eq!(Channel::from_setting(Some("stable")), Channel::Stable);
        assert_eq!(Channel::from_setting(Some("Nightly")), Channel::Nightly);
        assert_eq!(Channel::from_setting(Some("beta")), Channel::Stable);
    }

    #[test]
    fn the_latest_release_is_read_and_compared() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let seen = check(&latest_is(r#"{"tag_name": "v1.6.0", "html_url": "x", "draft": false, "prerelease": false}"#), now, Channel::Stable).unwrap();
        assert_eq!(seen, Seen { checked: 1_000_000, latest: Some("v1.6.0".into()), nightly: None });
        let newer = |running: &str| seen.newer_than(&v(running), None, Channel::Stable);
        assert_eq!(newer("1.5.0"), Some(Newer::Release(v("1.6.0"))));
        assert_eq!(newer("1.6.0"), None, "the same one");
        assert_eq!(newer("1.7.0"), None, "an older one");
        assert_eq!(newer("1.6.0-nightly.20261112"), Some(Newer::Release(v("1.6.0"))), "a nightly's release");
        assert_eq!(newer("1.7.0-nightly.20261201"), None, "a nightly past it");
        assert!(check(&latest_is(r#"{"tag_name": "v1.7.0-beta", "prerelease": true}"#), now, Channel::Stable).is_err());
        assert!(check(&latest_is("<html>rate limited</html>"), now, Channel::Stable).is_err());
    }

    #[test]
    fn the_nightly_is_read_from_its_record() {
        let n = nightly(&Answers(vec![(NIGHTLY_URL, NIGHTLY)])).unwrap();
        assert_eq!(n, Nightly { version: "1.6.0-nightly.20261112".into(), commit: Some("0123456789abcdef0123456789abcdef01234567".into()) });
        // No record: the title will do, with no commit.
        let n = nightly(&Answers(vec![(NIGHTLY_URL, r#"{"tag_name": "nightly", "name": "Fenix 1.6.0-nightly.20261113", "body": "notes"}"#)])).unwrap();
        assert_eq!(n, Nightly { version: "1.6.0-nightly.20261113".into(), commit: None });
        assert!(nightly(&Answers(vec![(NIGHTLY_URL, r#"{"tag_name": "nightly", "name": "Fenix nightly"}"#)])).is_err(), "no version");
        assert!(nightly(&Answers(vec![(NIGHTLY_URL, r#"{"tag_name": "v1.6.0", "name": "Fenix 1.6.0-nightly.1"}"#)])).is_err(), "not the nightly");
        assert!(nightly(&Answers(vec![])).is_err(), "none yet");
    }

    #[test]
    fn nightlies_are_told_about_only_on_the_nightly_channel() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let github = Answers(vec![(LATEST_URL, r#"{"tag_name": "v1.5.2"}"#), (NIGHTLY_URL, NIGHTLY)]);
        assert_eq!(check(&github, now, Channel::Stable).unwrap().nightly, None, "not even asked for");
        let seen = check(&github, now, Channel::Nightly).unwrap();
        assert_eq!(seen.latest.as_deref(), Some("v1.5.2"));
        let commit = "0123456789abcdef0123456789abcdef01234567";
        let tonight = Newer::Nightly { version: v("1.6.0-nightly.20261112"), commit: Some(commit.into()) };
        assert_eq!(seen.newer_than(&v("1.5.2"), None, Channel::Nightly), Some(tonight.clone()), "from a release");
        assert_eq!(seen.newer_than(&v("1.6.0-nightly.20261111"), Some("ffff"), Channel::Nightly), Some(tonight), "from last night's");
        assert_eq!(seen.newer_than(&v("1.6.0-nightly.20261112"), Some(commit), Channel::Nightly), None, "the one running");
        assert_eq!(seen.newer_than(&v("1.6.0-nightly.20261111"), Some(&commit.to_uppercase()), Channel::Nightly), None, "built from the commit running");
        assert_eq!(seen.newer_than(&v("1.5.2"), None, Channel::Stable), None, "back on stable: releases alone");
        assert_eq!(seen.newer_than(&v("1.5.1"), None, Channel::Stable), Some(Newer::Release(v("1.5.2"))));
    }

    #[test]
    fn a_release_newer_than_the_nightly_running_is_still_offered() {
        let seen = Seen { checked: 1, latest: Some("v1.6.0".into()), nightly: Some(Nightly { version: "1.6.0-nightly.20261112".into(), commit: None }) };
        assert_eq!(seen.newer_than(&v("1.6.0-nightly.20261112"), None, Channel::Nightly), Some(Newer::Release(v("1.6.0"))));
        assert_eq!(seen.newer_than(&v("1.5.2"), None, Channel::Nightly), Some(Newer::Release(v("1.6.0"))), "the newer of the two");
        // The next nightly is past the release.
        let seen = Seen { nightly: Some(Nightly { version: "1.7.0-nightly.20261201".into(), commit: None }), ..seen };
        assert!(matches!(seen.newer_than(&v("1.6.0"), None, Channel::Nightly), Some(Newer::Nightly { .. })));
    }

    #[test]
    fn either_answer_alone_will_do_on_the_nightly_channel() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(5);
        let seen = check(&Answers(vec![(NIGHTLY_URL, NIGHTLY)]), now, Channel::Nightly).unwrap();
        assert_eq!((seen.latest, seen.nightly.is_some()), (None, true));
        let seen = check(&latest_is(r#"{"tag_name": "v1.5.2"}"#), now, Channel::Nightly).unwrap();
        assert_eq!((seen.latest.as_deref(), seen.nightly), (Some("v1.5.2"), None));
        assert!(check(&Answers(vec![]), now, Channel::Nightly).is_err());
    }

    #[test]
    fn an_older_file_reads_with_no_nightly() {
        let seen: Seen = serde_json::from_str(r#"{"checked": 3, "latest": "v1.5.2"}"#).unwrap();
        assert_eq!(seen, Seen { checked: 3, latest: Some("v1.5.2".into()), nightly: None });
        assert_eq!(serde_json::to_string(&seen).unwrap(), r#"{"checked":3,"latest":"v1.5.2"}"#, "written as before");
        // What master briefly kept for pre-releases is passed over.
        let seen: Seen = serde_json::from_str(r#"{"checked": 3, "latest": "v1.5.2", "prerelease": "v1.6.0-rc.1"}"#).unwrap();
        assert_eq!(seen.nightly, None);
    }

    #[test]
    fn the_check_is_due_once_a_day() {
        let at = |s: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let never = Seen::default();
        assert!(never.due(at(5)));
        let seen = Seen { checked: 100, latest: None, nightly: None };
        assert!(!seen.due(at(100 + EVERY.as_secs() - 1)));
        assert!(seen.due(at(100 + EVERY.as_secs())));
        assert!(seen.due(at(50)), "a clock that went back");
        assert_eq!(seen.next(at(200)), at(100 + EVERY.as_secs()));
    }

    #[test]
    fn what_was_seen_is_kept_and_a_broken_file_is_never_looked() {
        let dir = std::env::temp_dir().join(format!("fenix-update-test-{}", std::process::id()));
        let path = dir.join("updates.json");
        let seen = Seen { checked: 7, latest: Some("v2.0.0".into()), nightly: Some(Nightly { version: "2.1.0-nightly.20270101".into(), commit: Some("abcdef0".into()) }) };
        seen.write(&path).unwrap();
        assert_eq!(Seen::read(&path), seen);
        std::fs::write(&path, "{nope").unwrap();
        assert_eq!(Seen::read(&path), Seen::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
