#!/usr/bin/env bash
# Seeds the dev GitLab instance with everything Fenix's Merge Requests
# view needs to be exercised against a real server:
#
#   - a personal access token with a fixed value, so config.ini can be
#     written once and never touched again
#   - a project (`fenix-dev/widget`) with a real commit history
#   - two merge requests, one of them a draft, both with a pipeline-less
#     but otherwise complete shape
#   - a review thread anchored to a real diff line, and a plain comment
#     on the request as a whole -- the two cases the view renders in
#     different places
#
# Idempotent: run it again and it recreates the project from scratch.
# Everything here is throwaway, and the credentials are deliberately
# fixed and public (see docker-compose.yml).
set -euo pipefail

URL="${GITLAB_URL:-http://localhost:8929}"
TOKEN="${GITLAB_TOKEN:-fenix-dev-token-0123456789}"
CONTAINER="${GITLAB_CONTAINER:-fenix-gitlab}"
GROUP=fenix-dev
PROJECT=widget

api() {
  local method=$1 path=$2
  shift 2
  curl -sS -X "$method" -H "PRIVATE-TOKEN: $TOKEN" -H 'Content-Type: application/json' "$URL/api/v4$path" "$@"
}

# Pulls one field out of a JSON response on stdin. Python rather than
# `grep -o '"id":[0-9]*' | head -1`, which happens to work on some of
# these responses and silently picks the wrong `id` on others -- a
# project payload has an `id` on its namespace, its owner and its
# statistics too, and which one comes first is not something to rely on.
field() {
  python -c "import json,sys
try:
    d = json.load(sys.stdin)
except ValueError:
    sys.exit(1)
v = d
for key in '$1'.split('.'):
    if not isinstance(v, dict) or key not in v:
        sys.exit(1)
    v = v[key]
print(v)"
}

# --- 1. A token with a value we choose ------------------------------------
# Created through `rails runner` rather than the API, because every API
# call needs a token already -- this is the bootstrap. `gitlab-rails` is
# slow to start (~30s), so this is the one place the script pauses.
echo "==> creating the access token (this takes ~30s)"
docker exec -i "$CONTAINER" gitlab-rails runner - <<RUBY
user = User.find_by_username('root')
user.personal_access_tokens.where(name: 'fenix-dev').destroy_all
token = user.personal_access_tokens.create!(
  name: 'fenix-dev',
  scopes: [:api, :read_repository, :write_repository],
  expires_at: 365.days.from_now
)
token.set_token('$TOKEN')
token.save!
puts "token ready for #{user.username}"
RUBY

# --- 2. A clean project ---------------------------------------------------
echo "==> recreating $GROUP/$PROJECT"
existing=$(api GET "/projects/$GROUP%2F$PROJECT" | field id || true)
if [ -n "${existing:-}" ]; then
  api DELETE "/projects/$existing" >/dev/null
  # Deletion is asynchronous; the name stays taken for a moment after.
  for _ in $(seq 30); do
    sleep 2
    code=$(curl -s -o /dev/null -w '%{http_code}' -H "PRIVATE-TOKEN: $TOKEN" "$URL/api/v4/projects/$GROUP%2F$PROJECT")
    [ "$code" = "404" ] && break
  done
fi

group_id=$(api GET "/groups/$GROUP" | field id || true)
if [ -z "${group_id:-}" ]; then
  group_id=$(api POST "/groups" -d "{\"name\":\"$GROUP\",\"path\":\"$GROUP\",\"visibility\":\"private\"}" | field id)
fi
project_id=$(api POST "/projects" \
  -d "{\"name\":\"$PROJECT\",\"path\":\"$PROJECT\",\"namespace_id\":$group_id,\"initialize_with_readme\":false,\"visibility\":\"private\"}" |
  field id)
echo "    project id $project_id"

# --- 3. Real history, pushed over HTTP ------------------------------------
echo "==> pushing a history"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"
git init -q -b main
git config user.email 'dev@fenix.test'
git config user.name 'Fenix Dev'
git config http.sslVerify false

cat >widget.rs <<'EOF'
fn main() {
    let timeout = 30;
    println!("starting with timeout {timeout}");
}
EOF
cat >README.md <<'EOF'
# widget

A throwaway project for exercising Fenix's GitLab integration.
EOF
git add .
git commit -q -m 'Initial commit'
git remote add origin "http://root:$TOKEN@localhost:8929/$GROUP/$PROJECT.git"
git push -q origin main

# A branch that changes one line, so the diff has an added line, a
# removed line and context around both -- every case the review pane
# anchors a comment to.
git checkout -q -b feature/configurable-timeout
cat >widget.rs <<'EOF'
fn main() {
    let timeout = read_timeout();
    println!("starting with timeout {timeout}");
}

fn read_timeout() -> u64 {
    std::env::var("TIMEOUT").ok().and_then(|v| v.parse().ok()).unwrap_or(30)
}
EOF
git commit -q -am 'Make the timeout configurable'
git push -q origin feature/configurable-timeout

git checkout -q -b docs/expand-readme main
cat >>README.md <<'EOF'

## Running

There is nothing to run. That is the point.
EOF
git commit -q -am 'Expand the README'
git push -q origin docs/expand-readme
cd - >/dev/null

# --- 4. Merge requests ----------------------------------------------------
echo "==> opening merge requests"
mr=$(api POST "/projects/$project_id/merge_requests" -d '{
  "source_branch": "feature/configurable-timeout",
  "target_branch": "main",
  "title": "Make the timeout configurable",
  "description": "Reads `TIMEOUT` from the environment, falling back to the old hard-coded 30.\n\nWorth a look at the fallback: 30 was arbitrary before and still is."
}')
mr_iid=$(echo "$mr" | field iid)
echo "    !$mr_iid"

api POST "/projects/$project_id/merge_requests" -d '{
  "source_branch": "docs/expand-readme",
  "target_branch": "main",
  "title": "Draft: Expand the README",
  "description": "Not finished."
}' >/dev/null

# --- 5. A thread on a diff line, and one on the request itself ------------
# The position has to quote the merge request's own diff SHAs, which is
# exactly what Fenix does -- so if this succeeds, the payload shape Fenix
# sends is the one GitLab accepts.
echo "==> adding review threads"
# GitLab computes the diff asynchronously; `diff_refs` is null until
# it has, and a position quoting empty SHAs is rejected.
for attempt in $(seq 30); do
  refs=$(api GET "/projects/$project_id/merge_requests/$mr_iid")
  base=$(echo "$refs" | field diff_refs.base_sha || true)
  [ -n "${base:-}" ] && break
  sleep 2
done
head=$(echo "$refs" | field diff_refs.head_sha)
start=$(echo "$refs" | field diff_refs.start_sha)

api POST "/projects/$project_id/merge_requests/$mr_iid/discussions" -d "{
  \"body\": \"Should this be a const rather than a magic number?\",
  \"position\": {
    \"base_sha\": \"$base\", \"head_sha\": \"$head\", \"start_sha\": \"$start\",
    \"position_type\": \"text\",
    \"old_path\": \"widget.rs\", \"new_path\": \"widget.rs\",
    \"new_line\": 7
  }
}" >/dev/null

api POST "/projects/$project_id/merge_requests/$mr_iid/discussions" \
  -d '{"body": "Overall this reads well. One question inline."}' >/dev/null

cat <<EOF

Seeded. Put this in config.ini:

  [gitlab]
  base_url = $URL
  token = $TOKEN

Then clone the project and open it in Fenix:

  git clone http://root:$TOKEN@localhost:8929/$GROUP/$PROJECT.git
EOF
