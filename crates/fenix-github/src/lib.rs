//! `fenix-forge`'s `Forge` trait, over GitHub's REST API -- and its
//! GraphQL API for the two things REST can't do: reading review threads
//! with their resolved state, and resolving them.
//!
//! Shaped like `fenix-gitlab`: `ureq`, one authenticated-request helper
//! per verb, raw `serde_json::Value` read by `parse`. The token comes
//! from the GitHub CLI (`gh auth token`) when it's signed in, else from
//! `[github] token`; which repository is read from `origin`.

pub mod parse;

use fenix_forge::{Approvals, ChangedFile, Check, Discussion, DraftComment, Forge, MergeOptions, MergeRequest, MrFilter, NewRequest, Position, Verdict};
use serde_json::{json, Value};

const API: &str = "https://api.github.com";
const PER_PAGE: usize = 100;

pub struct GitHub {
    api: String,
    token: String,
    owner: String,
    repo: String,
    /// `owner/repo`, for a heading.
    full: String,
}

/// `owner/repo` from a GitHub remote URL -- SSH, `ssh://` or HTTPS --
/// or `None` when it isn't one.
pub fn repository(remote_url: &str) -> Option<(String, String)> {
    let url = remote_url.trim();
    let rest = if let Some(rest) = url.split_once("github.com").map(|(_, rest)| rest) {
        rest.trim_start_matches([':', '/'])
    } else {
        return None;
    };
    let path = rest.trim_end_matches('/').trim_end_matches(".git");
    let (owner, repo) = path.split_once('/')?;
    (!owner.is_empty() && !repo.is_empty() && !repo.contains('/')).then(|| (owner.to_string(), repo.to_string()))
}

/// The GitHub CLI's token, when it's signed in.
pub fn gh_token() -> Option<String> {
    #[allow(unused_mut)]
    let mut command = std::process::Command::new("gh");
    command.args(["auth", "token"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let out = command.output().ok().filter(|o| o.status.success())?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|t| !t.is_empty())
}

impl GitHub {
    pub fn new(token: impl Into<String>, owner: impl Into<String>, repo: impl Into<String>) -> Self {
        let (owner, repo) = (owner.into(), repo.into());
        GitHub { api: API.to_string(), token: token.into(), full: format!("{owner}/{repo}"), owner, repo }
    }

    /// A client for the repository `remote_url` names.
    pub fn from_remote(token: impl Into<String>, remote_url: &str) -> Option<Self> {
        repository(remote_url).map(|(owner, repo)| GitHub::new(token, owner, repo))
    }

    fn request(&self, method: &str, url: &str) -> ureq::Request {
        ureq::request(method, url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Accept", "application/vnd.github+json")
            .set("X-GitHub-Api-Version", "2022-11-28")
            .set("User-Agent", "fenix")
    }

    fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
        let mut req = self.request("GET", &format!("{}{path}", self.api));
        for (k, v) in query {
            req = req.query(k, v);
        }
        let response = req.call().map_err(describe_error)?;
        let text = response.into_string().map_err(|e| format!("couldn't read response body: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("couldn't parse response as JSON: {e}"))
    }

    /// Every page of a listing, up to a sane limit.
    fn get_all(&self, path: &str, query: &[(&str, &str)]) -> Result<Vec<Value>, String> {
        let mut out = Vec::new();
        for page in 1..=30 {
            let page = page.to_string();
            let per_page = PER_PAGE.to_string();
            let mut q: Vec<(&str, &str)> = query.to_vec();
            q.push(("per_page", &per_page));
            q.push(("page", &page));
            let value = self.get(path, &q)?;
            let items = value.as_array().cloned().unwrap_or_default();
            let done = items.len() < PER_PAGE;
            out.extend(items);
            if done {
                break;
            }
        }
        Ok(out)
    }

    fn send(&self, method: &str, path: &str, body: &Value) -> Result<Value, String> {
        let response = self.request(method, &format!("{}{path}", self.api)).send_string(&body.to_string()).map_err(describe_error)?;
        let text = response.into_string().map_err(|e| format!("couldn't read response body: {e}"))?;
        // 204 No Content and friends: fine.
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    fn graphql(&self, query: &str, variables: Value) -> Result<Value, String> {
        let value = self.send("POST", "/graphql", &json!({ "query": query, "variables": variables }))?;
        if let Some(errors) = value.get("errors").and_then(Value::as_array).filter(|e| !e.is_empty()) {
            let messages: Vec<&str> = errors.iter().filter_map(|e| e.get("message").and_then(Value::as_str)).collect();
            return Err(format!("GitHub said: {}", messages.join("; ")));
        }
        Ok(value.get("data").cloned().unwrap_or(Value::Null))
    }

    fn repo_path(&self, suffix: &str) -> String {
        format!("/repos/{}/{}{suffix}", self.owner, self.repo)
    }

    fn pull_path(&self, number: u64, suffix: &str) -> String {
        self.repo_path(&format!("/pulls/{number}{suffix}"))
    }

    fn node_id(&self, number: u64) -> Result<String, String> {
        let pull = self.get(&self.pull_path(number, ""), &[])?;
        pull.get("node_id").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "GitHub gave the pull request no node id".to_string())
    }
}

impl Forge for GitHub {
    fn project(&self) -> &str {
        &self.full
    }

    fn list_merge_requests(&self, filter: MrFilter) -> Result<Vec<MergeRequest>, String> {
        let pulls = self.get_all(&self.repo_path("/pulls"), &[("state", "open"), ("sort", "updated"), ("direction", "desc")])?;
        let me = match filter {
            MrFilter::AllOpen => String::new(),
            _ => self.current_user()?,
        };
        Ok(pulls
            .iter()
            .filter(|p| match filter {
                MrFilter::AllOpen => true,
                MrFilter::Mine => p.get("user").and_then(|u| u.get("login")).and_then(Value::as_str) == Some(me.as_str()),
                MrFilter::ReviewRequested => parse::requested_reviewers(p).contains(&me),
                MrFilter::ForMe => parse::requested_reviewers(p).contains(&me) || parse::assignees(p).contains(&me),
            })
            .filter_map(parse::pull_request)
            .collect())
    }

    fn merge_request(&self, number: u64) -> Result<MergeRequest, String> {
        let value = self.get(&self.pull_path(number, ""), &[])?;
        let mut pr = parse::pull_request(&value).ok_or_else(|| format!("#{number} came back without a number"))?;
        if let Ok(checks) = self.checks(number, &pr.sha.clone()) {
            pr.pipeline = parse::overall(&checks);
        }
        Ok(pr)
    }

    fn approvals(&self, number: u64) -> Result<Approvals, String> {
        let reviews = self.get_all(&self.pull_path(number, "/reviews"), &[])?;
        Ok(parse::approvals(&Value::Array(reviews)))
    }

    fn changed_files(&self, number: u64) -> Result<Vec<ChangedFile>, String> {
        Ok(self.get_all(&self.pull_path(number, "/files"), &[])?.iter().filter_map(parse::changed_file).collect())
    }

    fn checkout_refspec(&self, number: u64) -> String {
        // GitHub publishes every pull request's head here, forks included.
        format!("refs/pull/{number}/head:mr-{number}")
    }

    fn discussions(&self, number: u64) -> Result<Vec<Discussion>, String> {
        const QUERY: &str = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){\
            reviewThreads(first:100){nodes{id isResolved isOutdated path line originalLine diffSide \
            comments(first:100){nodes{databaseId author{login} body createdAt}}}} \
            comments(first:100){nodes{databaseId author{login} body createdAt}}}}}";
        let data = self.graphql(QUERY, json!({ "owner": self.owner, "name": self.repo, "number": number }))?;
        let pull = data.get("repository").and_then(|r| r.get("pullRequest")).ok_or_else(|| format!("no pull request #{number}"))?;
        Ok(parse::discussions(pull))
    }

    fn reply(&self, number: u64, discussion: &str, body: &str) -> Result<(), String> {
        // The conversation under the description isn't a thread: a reply
        // there is one more comment on it.
        if discussion.starts_with("issue-") {
            return self.send("POST", &self.repo_path(&format!("/issues/{number}/comments")), &json!({ "body": body })).map(|_| ());
        }
        const MUTATION: &str = "mutation($id:ID!,$body:String!){addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId:$id,body:$body}){comment{id}}}";
        self.graphql(MUTATION, json!({ "id": discussion, "body": body })).map(|_| ())
    }

    fn resolve(&self, _number: u64, discussion: &str, resolved: bool) -> Result<(), String> {
        let mutation = if resolved {
            "mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{isResolved}}}"
        } else {
            "mutation($id:ID!){unresolveReviewThread(input:{threadId:$id}){thread{isResolved}}}"
        };
        self.graphql(mutation, json!({ "id": discussion })).map(|_| ())
    }

    fn comment_on_line(&self, number: u64, position: &Position, body: &str) -> Result<(), String> {
        let (path, line, side) = anchor(position);
        let head = if position.head_sha.is_empty() { self.merge_request(number)?.sha } else { position.head_sha.clone() };
        let payload = json!({ "body": body, "commit_id": head, "path": path, "line": line, "side": side });
        self.send("POST", &self.pull_path(number, "/comments"), &payload).map(|_| ())
    }

    fn approve(&self, number: u64, sha: Option<&str>) -> Result<(), String> {
        let mut body = json!({ "event": "APPROVE" });
        if let Some(sha) = sha {
            body["commit_id"] = json!(sha);
        }
        self.send("POST", &self.pull_path(number, "/reviews"), &body).map(|_| ())
    }

    fn unapprove(&self, _number: u64) -> Result<(), String> {
        Err("GitHub doesn't let a reviewer withdraw an approval -- submit a review that requests changes instead".to_string())
    }

    fn merge(&self, number: u64, options: &MergeOptions) -> Result<(), String> {
        let method = if options.squash {
            "squash"
        } else if options.rebase {
            "rebase"
        } else {
            "merge"
        };
        if options.when_checks_pass {
            const MUTATION: &str = "mutation($id:ID!,$method:PullRequestMergeMethod!){enablePullRequestAutoMerge(input:{pullRequestId:$id,mergeMethod:$method}){pullRequest{number}}}";
            let id = self.node_id(number)?;
            return self.graphql(MUTATION, json!({ "id": id, "method": method.to_uppercase() })).map(|_| ());
        }
        let pull = self.get(&self.pull_path(number, ""), &[])?;
        let mut body = json!({ "merge_method": method });
        if let Some(sha) = &options.sha {
            body["sha"] = json!(sha);
        }
        self.send("PUT", &self.pull_path(number, "/merge"), &body)?;
        if options.remove_source_branch {
            // Only a branch in this repository, never a fork's.
            let same_repo = pull.get("head").and_then(|h| h.get("repo")).and_then(|r| r.get("full_name")).and_then(Value::as_str) == Some(self.full.as_str());
            if let (true, Some(branch)) = (same_repo, pull.get("head").and_then(|h| h.get("ref")).and_then(Value::as_str)) {
                self.send("DELETE", &self.repo_path(&format!("/git/refs/heads/{branch}")), &Value::Null)?;
            }
        }
        Ok(())
    }

    fn current_user(&self) -> Result<String, String> {
        let user = self.get("/user", &[])?;
        user.get("login").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "GitHub didn't say who the token belongs to".to_string())
    }

    fn request_for_branch(&self, branch: &str) -> Result<Option<MergeRequest>, String> {
        let head = format!("{}:{branch}", self.owner);
        let found = self.get(&self.repo_path("/pulls"), &[("state", "open"), ("head", &head)])?;
        match found.as_array().and_then(|l| l.first()).and_then(|p| p.get("number")).and_then(Value::as_u64) {
            Some(number) => self.merge_request(number).map(Some),
            None => Ok(None),
        }
    }

    fn create_request(&self, request: &NewRequest) -> Result<MergeRequest, String> {
        let body = json!({
            "title": request.title,
            "head": request.source_branch,
            "base": request.target_branch,
            "body": request.description,
            "draft": request.draft,
        });
        let value = self.send("POST", &self.repo_path("/pulls"), &body)?;
        parse::pull_request(&value).ok_or_else(|| "GitHub didn't say what it made".to_string())
    }

    fn submit_review(&self, number: u64, head_sha: &str, verdict: Verdict, body: &str, comments: &[DraftComment]) -> Result<(), String> {
        let event = match verdict {
            Verdict::Approve => "APPROVE",
            Verdict::RequestChanges => "REQUEST_CHANGES",
            Verdict::Comment => "COMMENT",
        };
        // GitHub wants words with anything but an approval.
        let body = match (verdict, body.trim().is_empty(), comments.is_empty()) {
            (Verdict::Approve, _, _) | (_, false, _) => body.to_string(),
            (Verdict::RequestChanges, true, _) => "Requesting changes -- see the comments.".to_string(),
            (Verdict::Comment, true, false) => "Comments inline.".to_string(),
            (Verdict::Comment, true, true) => return Err("nothing to send: no comments and no summary".to_string()),
        };
        let comments: Vec<Value> = comments
            .iter()
            .map(|c| {
                let (path, line, side) = anchor(&c.position);
                let mut out = json!({ "path": path, "line": line, "side": side, "body": c.body });
                if let Some(start) = c.start_line.filter(|s| *s < line) {
                    out["start_line"] = json!(start);
                    out["start_side"] = json!(side);
                }
                out
            })
            .collect();
        let payload = json!({ "commit_id": head_sha, "body": body, "event": event, "comments": comments });
        self.send("POST", &self.pull_path(number, "/reviews"), &payload).map(|_| ())
    }

    fn request_review(&self, number: u64, users: &[String]) -> Result<(), String> {
        self.send("POST", &self.pull_path(number, "/requested_reviewers"), &json!({ "reviewers": users })).map(|_| ())
    }

    fn checks(&self, _number: u64, head_sha: &str) -> Result<Vec<Check>, String> {
        let value = self.get(&self.repo_path(&format!("/commits/{head_sha}/check-runs")), &[("per_page", "100")])?;
        Ok(value.get("check_runs").and_then(Value::as_array).map(|l| l.iter().filter_map(parse::check_run).collect()).unwrap_or_default())
    }

    fn job_log(&self, check: &Check) -> Result<String, String> {
        // An Actions job's log; GitHub redirects to a signed download.
        let response = self.request("GET", &format!("{}{}", self.api, self.repo_path(&format!("/actions/jobs/{}/logs", check.id)))).call().map_err(describe_error)?;
        response.into_string().map_err(|e| format!("couldn't read the log: {e}"))
    }

    fn retry(&self, check: &Check) -> Result<(), String> {
        self.send("POST", &self.repo_path(&format!("/actions/jobs/{}/rerun", check.id)), &json!({})).map(|_| ())
    }
}

/// Where a comment goes, GitHub's way: the path, the line and the side
/// (`RIGHT` for the new version, `LEFT` for a removed line).
fn anchor(position: &Position) -> (String, usize, &'static str) {
    match (position.new_line, position.old_line) {
        (Some(new), _) => (position.new_path.clone(), new, "RIGHT"),
        (None, Some(old)) => (position.old_path.clone(), old, "LEFT"),
        (None, None) => (position.new_path.clone(), 1, "RIGHT"),
    }
}

/// A status and GitHub's own message ("Can not approve your own pull
/// request"), which says more than any code.
fn describe_error(err: ureq::Error) -> String {
    match err {
        ureq::Error::Status(code, response) => {
            let status = response.status_text().to_string();
            let body = response.into_string().unwrap_or_default();
            let message = serde_json::from_str::<Value>(&body).ok().and_then(|v| {
                let top = v.get("message").and_then(Value::as_str).map(str::to_string);
                let detail = v.get("errors").and_then(Value::as_array).and_then(|e| e.first()).and_then(|e| e.get("message").or(Some(e))).map(|m| m.as_str().map(str::to_string).unwrap_or_else(|| m.to_string()));
                match (top, detail) {
                    (Some(t), Some(d)) => Some(format!("{t}: {d}")),
                    (t, d) => t.or(d),
                }
            });
            match message {
                Some(m) => format!("HTTP {code} ({status}): {m}"),
                None => format!("HTTP {code} ({status})"),
            }
        }
        ureq::Error::Transport(transport) => format!("request failed: {transport}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_repository_is_read_from_every_kind_of_remote() {
        let want = Some(("tpedneault".to_string(), "fenix".to_string()));
        assert_eq!(repository("https://github.com/tpedneault/fenix.git"), want);
        assert_eq!(repository("https://github.com/tpedneault/fenix"), want);
        assert_eq!(repository("git@github.com:tpedneault/fenix.git"), want);
        assert_eq!(repository("ssh://git@github.com/tpedneault/fenix.git"), want);
        assert_eq!(repository("https://gitlab.example.com/g/p.git"), None);
    }

    #[test]
    fn a_comment_anchors_on_the_new_side_unless_it_is_a_removed_line() {
        let base = Position { base_sha: String::new(), head_sha: String::new(), start_sha: String::new(), old_path: "a".into(), new_path: "b".into(), old_line: Some(3), new_line: Some(4) };
        assert_eq!(anchor(&base), ("b".to_string(), 4, "RIGHT"));
        let removed = Position { new_line: None, ..base };
        assert_eq!(anchor(&removed), ("a".to_string(), 3, "LEFT"));
    }

    #[test]
    fn the_checkout_ref_is_githubs_own() {
        assert_eq!(GitHub::new("t", "o", "r").checkout_refspec(7), "refs/pull/7/head:mr-7");
    }
}
