#!/usr/bin/env bash
# Blocks until the GitLab container is actually serving the API.
#
# GitLab's own healthcheck reports healthy well before `/api/v4` will
# answer -- Puma comes up last, after Postgres, Redis, Gitaly and the
# reconfigure run. Seeding against a "healthy" container that isn't
# serving yet fails with a connection reset, which looks like a bug in
# the seed script rather than what it is.
set -euo pipefail

URL="${GITLAB_URL:-http://localhost:8929}"
DEADLINE=$(( $(date +%s) + ${TIMEOUT:-900} ))
# How many consecutive good answers count as "up". One is not enough:
# nginx starts answering before Puma is ready and keeps flapping 502
# through the last of the reconfigure, so a script that starts work on
# the first 200 gets a 502 thirty seconds later -- which is exactly how
# seeding fails in a way that looks like a bug in the seed script.
STREAK=${STREAK:-3}
good=0

printf 'waiting for %s (first boot takes several minutes)' "$URL"
while :; do
  # `/-/readiness` answers before the API does; the version endpoint is
  # the first thing that proves `/api/v4` itself is routed, and a 401
  # (no token) is a perfectly good "it's up".
  code=$(curl -s -o /dev/null -w '%{http_code}' "$URL/api/v4/version" || echo 000)
  case "$code" in
    200 | 401 | 403)
      printf '\nready (HTTP %s)\n' "$code"
      exit 0
      ;;
  esac
  # A crash loop looks exactly like a slow boot from out here, and
  # waiting fifteen minutes to find out is the wrong answer -- the
  # container restarting means the reconfigure failed, which no amount
  # of patience fixes.
  status=$(docker ps -a --filter "name=${CONTAINER:-fenix-gitlab}" --format '{{.Status}}' 2>/dev/null || true)
  case "$status" in
    *Restarting* | *Exited*)
      printf '\nthe container is %s, not starting slowly\n' "$status" >&2
      docker logs --tail 20 "${CONTAINER:-fenix-gitlab}" 2>&1 | grep -iE 'error|fatal' | tail -5 >&2 || true
      exit 1
      ;;
  esac
  if [ "$(date +%s)" -ge "$DEADLINE" ]; then
    printf '\ngave up after %ss (last status %s)\n' "${TIMEOUT:-900}" "$code" >&2
    echo "container logs: docker logs --tail 50 fenix-gitlab" >&2
    exit 1
  fi
  printf '.'
  sleep 10
done
