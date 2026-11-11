//! Whether a newer Fenix is out: GitHub's latest release of
//! `tpedneault/fenix`, read at most once a day when `updates.check` is
//! on, or when asked. Nothing is downloaded or installed -- Home says
//! there's a newer one, and where.
//!
//! When it last looked, and what it found, is kept in a small file of
//! Fenix's state (`updates.json`), so a restart doesn't look again.
//!
//! Pre-releases (`v1.6.0-rc.1`, published on GitHub as pre-releases) are
//! told about only with `updates.prereleases` on: then the check reads
//! the recent releases rather than the latest, and keeps the newest
//! release and the newest pre-release apart, so turning it off again goes
//! back to releases alone without looking again.

use std::path::Path;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::registry::Fetch;

/// Where the latest release is read.
pub const LATEST_URL: &str = "https://api.github.com/repos/tpedneault/fenix/releases/latest";

/// Where the recent releases are read, pre-releases among them
/// (`updates.prereleases`). GitHub lists the newest first.
pub const RECENT_URL: &str = "https://api.github.com/repos/tpedneault/fenix/releases?per_page=10";

/// Where people are sent to get one.
pub const RELEASES_PAGE: &str = "https://github.com/tpedneault/fenix/releases";

/// How often the daily check looks.
pub const EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// The most of GitHub's answer that's read.
const MAX_ANSWER: u64 = 512 * 1024;

/// What was found when it last looked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    /// When, in seconds since 1970.
    #[serde(default)]
    pub checked: u64,
    /// The newest release then, as GitHub tags it (`v1.6.0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
    /// The newest pre-release then (`v1.7.0-rc.1`), when it's newer than
    /// `latest` and `updates.prereleases` was on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prerelease: Option<String>,
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

    /// The newer release than `running` it found, if any; with
    /// `prereleases`, a newer pre-release counts too.
    pub fn newer_than(&self, running: &semver::Version, prereleases: bool) -> Option<semver::Version> {
        let release = self.latest.as_deref().and_then(release_version);
        let pre = if prereleases { self.prerelease.as_deref().and_then(any_version) } else { None };
        release.into_iter().chain(pre).max().filter(|v| v > running)
    }
}

/// A release's version from its tag: `v1.6.0` or `1.6.0`. A pre-release
/// (`v1.6.0-rc.1`) isn't one anyone's told about.
pub fn release_version(tag: &str) -> Option<semver::Version> {
    let v = semver::Version::parse(tag.trim().trim_start_matches('v')).ok()?;
    v.pre.is_empty().then_some(v)
}

/// Any version from a tag, a pre-release's too (`v1.6.0-rc.1`).
pub fn any_version(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.trim().trim_start_matches('v')).ok()
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
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

/// The newest release's tag among the recent ones, and the newest
/// pre-release's when it's newer than that.
pub fn recent(fetch: &dyn Fetch) -> Result<(Option<String>, Option<String>), String> {
    let bytes = fetch.get(RECENT_URL, MAX_ANSWER)?;
    let releases: Vec<Release> = serde_json::from_slice(&bytes).map_err(|e| format!("GitHub's answer didn't read: {e}"))?;
    let newest = |pre: bool| {
        releases
            .iter()
            .filter(|r| !r.draft)
            .filter_map(|r| {
                let v = any_version(&r.tag_name)?;
                // A release is one GitHub doesn't call a pre-release and
                // whose version doesn't either; a pre-release is either.
                let is_pre = r.prerelease || !v.pre.is_empty();
                (is_pre == pre).then_some((v, r.tag_name.clone()))
            })
            .max_by(|a, b| a.0.cmp(&b.0))
    };
    let release = newest(false);
    let pre = newest(true).filter(|(v, _)| release.as_ref().is_none_or(|(r, _)| v > r));
    if release.is_none() && pre.is_none() {
        return Err("GitHub lists no release to offer".into());
    }
    Ok((release.map(|(_, tag)| tag), pre.map(|(_, tag)| tag)))
}

/// Looks now: what was found, as of `now`; with `prereleases`, the
/// newest pre-release too.
pub fn check(fetch: &dyn Fetch, now: SystemTime, prereleases: bool) -> Result<Seen, String> {
    let checked = now.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if prereleases {
        let (latest, prerelease) = recent(fetch)?;
        return Ok(Seen { checked, latest, prerelease });
    }
    let tag = latest(fetch)?;
    Ok(Seen { checked, latest: Some(tag), prerelease: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Answer(&'static str);
    impl Fetch for Answer {
        fn get(&self, url: &str, _limit: u64) -> Result<Vec<u8>, String> {
            assert_eq!(url, LATEST_URL);
            Ok(self.0.as_bytes().to_vec())
        }
    }

    struct Recent(&'static str);
    impl Fetch for Recent {
        fn get(&self, url: &str, _limit: u64) -> Result<Vec<u8>, String> {
            assert_eq!(url, RECENT_URL);
            Ok(self.0.as_bytes().to_vec())
        }
    }

    fn v(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    #[test]
    fn a_release_s_tag_is_its_version_and_a_pre_release_is_none() {
        assert_eq!(release_version("v1.6.0"), Some(v("1.6.0")));
        assert_eq!(release_version("1.6.0"), Some(v("1.6.0")));
        assert_eq!(release_version("v1.6.0-rc.1"), None);
        assert_eq!(release_version("nightly"), None);
    }

    #[test]
    fn the_latest_release_is_read_and_compared() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let seen = check(&Answer(r#"{"tag_name": "v1.6.0", "html_url": "x", "draft": false, "prerelease": false}"#), now, false).unwrap();
        assert_eq!(seen, Seen { checked: 1_000_000, latest: Some("v1.6.0".into()), prerelease: None });
        assert_eq!(seen.newer_than(&v("1.5.0"), false), Some(v("1.6.0")));
        assert_eq!(seen.newer_than(&v("1.6.0"), false), None, "the same one");
        assert_eq!(seen.newer_than(&v("1.7.0"), false), None, "an older one");
        assert_eq!(seen.newer_than(&v("1.6.0-rc.2"), false), Some(v("1.6.0")), "a pre-release's release");
        assert!(check(&Answer(r#"{"tag_name": "v1.7.0-beta", "prerelease": true}"#), now, false).is_err());
        assert!(check(&Answer("<html>rate limited</html>"), now, false).is_err());
    }

    #[test]
    fn pre_releases_are_told_about_only_when_asked_for() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let list = Recent(
            r#"[
                {"tag_name": "v1.7.0-rc.2", "prerelease": true},
                {"tag_name": "v1.7.0-rc.1", "prerelease": true},
                {"tag_name": "v1.8.0", "draft": true},
                {"tag_name": "v1.6.1", "prerelease": false},
                {"tag_name": "v1.6.0"},
                {"tag_name": "nightly", "prerelease": true}
            ]"#,
        );
        let seen = check(&list, now, true).unwrap();
        assert_eq!(seen.latest.as_deref(), Some("v1.6.1"), "the newest release, not a draft");
        assert_eq!(seen.prerelease.as_deref(), Some("v1.7.0-rc.2"));
        assert_eq!(seen.newer_than(&v("1.6.1"), true), Some(v("1.7.0-rc.2")));
        assert_eq!(seen.newer_than(&v("1.7.0-rc.1"), true), Some(v("1.7.0-rc.2")));
        assert_eq!(seen.newer_than(&v("1.7.0-rc.2"), true), None, "the one running");
        assert_eq!(seen.newer_than(&v("1.6.1"), false), None, "turned off again: releases alone");
        assert_eq!(seen.newer_than(&v("1.6.0"), false), Some(v("1.6.1")));
        // A pre-release older than the newest release isn't kept.
        let seen = check(&Recent(r#"[{"tag_name": "v1.7.0"}, {"tag_name": "v1.7.0-rc.1", "prerelease": true}]"#), now, true).unwrap();
        assert_eq!((seen.latest.as_deref(), seen.prerelease), (Some("v1.7.0"), None));
        // A version that says it's a pre-release is one, whatever GitHub says.
        let seen = check(&Recent(r#"[{"tag_name": "v2.0.0-beta.1"}]"#), now, true).unwrap();
        assert_eq!((seen.latest, seen.prerelease.as_deref()), (None, Some("v2.0.0-beta.1")));
        assert!(check(&Recent("[]"), now, true).is_err());
        assert!(check(&Recent("<html>rate limited</html>"), now, true).is_err());
    }

    #[test]
    fn an_older_file_reads_with_no_pre_release() {
        let seen: Seen = serde_json::from_str(r#"{"checked": 3, "latest": "v1.5.2"}"#).unwrap();
        assert_eq!(seen, Seen { checked: 3, latest: Some("v1.5.2".into()), prerelease: None });
        assert_eq!(serde_json::to_string(&seen).unwrap(), r#"{"checked":3,"latest":"v1.5.2"}"#, "written as before");
    }

    #[test]
    fn the_check_is_due_once_a_day() {
        let at = |s: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let never = Seen::default();
        assert!(never.due(at(5)));
        let seen = Seen { checked: 100, latest: None, prerelease: None };
        assert!(!seen.due(at(100 + EVERY.as_secs() - 1)));
        assert!(seen.due(at(100 + EVERY.as_secs())));
        assert!(seen.due(at(50)), "a clock that went back");
        assert_eq!(seen.next(at(200)), at(100 + EVERY.as_secs()));
    }

    #[test]
    fn what_was_seen_is_kept_and_a_broken_file_is_never_looked() {
        let dir = std::env::temp_dir().join(format!("fenix-update-test-{}", std::process::id()));
        let path = dir.join("updates.json");
        let seen = Seen { checked: 7, latest: Some("v2.0.0".into()), prerelease: Some("v2.1.0-rc.1".into()) };
        seen.write(&path).unwrap();
        assert_eq!(Seen::read(&path), seen);
        std::fs::write(&path, "{nope").unwrap();
        assert_eq!(Seen::read(&path), Seen::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
