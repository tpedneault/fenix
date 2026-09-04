//! Working out which GitLab project a checkout belongs to, from the
//! `origin` remote it was cloned from.
//!
//! This exists so the only configuration is a base URL and a token.
//! Asking the user to also name the project per repo would be asking
//! them to restate something the checkout already knows -- and to keep
//! restating it for every repo they open.

/// The `group/subgroup/project` path in a GitLab remote URL, or `None`
/// for a URL this can't read.
///
/// Handles the three forms a `git remote -v` actually shows:
///
/// - `git@gitlab.example.com:group/sub/project.git` (SSH, scp-like)
/// - `ssh://git@gitlab.example.com:2222/group/sub/project.git`
/// - `https://gitlab.example.com/group/sub/project.git`
///
/// A `.git` suffix and any trailing slash come off; the host does not
/// have to match the configured base URL, because a GitLab instance is
/// routinely reached over a different hostname than the one it's
/// cloned from (an SSH alias, a proxy, a VPN-only name). Checking would
/// reject working setups to catch a misconfiguration that announces
/// itself as a 404 anyway.
pub fn project_path(remote_url: &str) -> Option<String> {
    let url = remote_url.trim();
    if url.is_empty() {
        return None;
    }
    // The scp-like form has no scheme and uses `:` to separate host
    // from path -- `git@host:group/project.git`. Distinguished from a
    // real URL by the absence of `://`, since `ssh://host:2222/path`
    // also contains a colon but means something else entirely.
    let path = if let Some(rest) = url.split_once("://").map(|(_, rest)| rest) {
        // Strip `user@host[:port]`, keeping everything past the first
        // `/`.
        rest.split_once('/').map(|(_, path)| path)?
    } else if let Some((_, path)) = url.split_once(':') {
        path
    } else {
        return None;
    };

    let path = path.trim_end_matches('/').trim_end_matches(".git").trim_matches('/');
    if path.is_empty() || !path.contains('/') {
        // A GitLab project always lives under at least one namespace,
        // so a single bare segment means this wasn't a project URL.
        return None;
    }
    Some(path.to_string())
}

/// A project path encoded for a GitLab API URL segment.
///
/// GitLab accepts either a numeric id or the URL-encoded full path in
/// the `:id` position, and the path is the one a checkout can work out
/// on its own. Only `/` has to be encoded in practice -- a project or
/// group slug is limited to letters, digits, `_`, `-`, and `.` -- but
/// anything outside that set is percent-encoded too rather than
/// trusting the constraint to hold.
pub fn encode_project(path: &str) -> String {
    let mut out = String::with_capacity(path.len() + 8);
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_scp_like_ssh_form() {
        assert_eq!(project_path("git@gitlab.example.com:group/project.git").as_deref(), Some("group/project"));
    }

    #[test]
    fn reads_a_nested_subgroup_path() {
        assert_eq!(
            project_path("git@gitlab.example.com:group/sub/deeper/project.git").as_deref(),
            Some("group/sub/deeper/project")
        );
    }

    #[test]
    fn reads_the_https_form() {
        assert_eq!(project_path("https://gitlab.example.com/group/project.git").as_deref(), Some("group/project"));
    }

    #[test]
    fn reads_an_ssh_url_with_a_port() {
        // The `:2222` here is a port, not the scp-like form's path
        // separator -- told apart by the `://`.
        assert_eq!(project_path("ssh://git@gitlab.example.com:2222/group/project.git").as_deref(), Some("group/project"));
    }

    #[test]
    fn a_missing_git_suffix_or_trailing_slash_is_fine() {
        assert_eq!(project_path("https://gitlab.example.com/group/project").as_deref(), Some("group/project"));
        assert_eq!(project_path("https://gitlab.example.com/group/project/").as_deref(), Some("group/project"));
    }

    #[test]
    fn a_url_with_no_namespace_is_rejected_rather_than_guessed_at() {
        // Every GitLab project sits under at least one group, so a
        // single segment means this isn't one.
        assert_eq!(project_path("https://example.com/project.git"), None);
        assert_eq!(project_path(""), None);
        assert_eq!(project_path("not a url"), None);
    }

    #[test]
    fn a_dot_in_a_project_name_is_not_mistaken_for_the_git_suffix() {
        assert_eq!(project_path("git@host:group/my.project.git").as_deref(), Some("group/my.project"));
    }

    #[test]
    fn encoding_escapes_the_path_separators_and_nothing_ordinary() {
        assert_eq!(encode_project("group/sub/project"), "group%2Fsub%2Fproject");
        assert_eq!(encode_project("my-group.name_1"), "my-group.name_1");
    }

    #[test]
    fn encoding_escapes_anything_outside_the_slug_character_set() {
        assert_eq!(encode_project("a b"), "a%20b");
    }
}
