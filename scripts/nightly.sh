#!/usr/bin/env bash
# What the nightly workflow (.github/workflows/nightly.yml) decides, as
# commands that can be tried and tested without it
# (scripts/nightly-test.sh):
#
#   scripts/nightly.sh version <workspace version> <YYYYMMDD> [<last nightly's version>]
#       The nightly's version: the next minor after the workspace's, a
#       pre-release of it stamped with the date -- 1.5.2 on 2026-11-12 is
#       1.6.0-nightly.20261112. A second nightly the same day (a forced
#       run) is 1.6.0-nightly.20261112.2, then .3: each sorts after the
#       one before, and every one before the 1.6.0 release, for SemVer,
#       dpkg and rpm alike.
#
#   scripts/nightly.sh needs-build <master's head> <last nightly's commit, or ""> [force]
#       "true" or "false", and why on stderr: true when master has a
#       commit the last nightly wasn't built from, or when forced.
#
#   scripts/nightly.sh record <release notes file>
#       The version and commit the notes' record says (the line `notes`
#       writes): "<version> <commit>", or nothing when there's none.
#
#   scripts/nightly.sh notes <version> <commit> <YYYY-MM-DD> <owner/repo> [<last commit>] < CHANGELOG.md
#       The release notes: what it is, the commit and date, the changes
#       since the last nightly, and the changelog's Unreleased section,
#       then the record the update check and the next run read.
set -euo pipefail

die() { echo "nightly.sh: $*" >&2; exit 2; }

# The nightly's version.
version() {
  local workspace=$1 date=$2 last=${3-}
  [[ $date =~ ^[0-9]{8}$ ]] || die "not a date (YYYYMMDD): $date"
  [[ $workspace =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]] || die "not a release version: $workspace"
  local base="${BASH_REMATCH[1]}.$((BASH_REMATCH[2] + 1)).0"
  local v="$base-nightly.$date"
  # The same day as the last: one more than its count (none is the first).
  if [[ $last == "$v" ]]; then
    v="$v.2"
  elif [[ $last =~ ^${base//./\\.}-nightly\.${date}\.([0-9]+)$ ]]; then
    v="$v.$((BASH_REMATCH[1] + 1))"
  fi
  echo "$v"
}

needs_build() {
  local head=$1 built=${2-} force=${3-}
  [[ $head =~ ^[0-9a-f]{40}$ ]] || die "not a commit: $head"
  if [[ $force == true ]]; then
    echo "forced: building $head" >&2
    echo true
  elif [[ -z $built ]]; then
    echo "no nightly yet: building $head" >&2
    echo true
  elif [[ $built == "$head" ]]; then
    echo "master is still at $head, the last nightly's commit: nothing to build" >&2
    echo false
  else
    echo "master moved from $built to $head: building" >&2
    echo true
  fi
}

record() {
  local file=$1 line
  line=$(grep -m1 -o '<!-- fenix-nightly [^>]*-->' "$file" || true)
  [[ -n $line ]] || return 0
  local version commit
  version=$(sed -n 's/.* version=\([^ ]*\).*/\1/p' <<< "$line")
  commit=$(sed -n 's/.* commit=\([0-9a-f]\{40\}\).*/\1/p' <<< "$line")
  [[ -n $version && -n $commit ]] && echo "$version $commit"
  return 0
}

# The changelog's Unreleased section, without its heading.
unreleased() {
  awk '
    /^## \[Unreleased\]/ { found = 1; next }
    /^## \[/ { if (found) exit }
    /^\[[^]]+\]: / { if (found) exit }
    found { print }
  '
}

notes() {
  local version=$1 commit=$2 date=$3 repo=$4 last=${5-}
  local changes
  changes=$(unreleased)
  cat <<EOF
The nightly build of Fenix: **$version**, built from
[\`${commit:0:12}\`](https://github.com/$repo/commit/$commit) on \`master\`
on $date. It's rebuilt each night \`master\` changed, and replaces the
one before; it isn't a release. Releases are on the
[releases page](https://github.com/$repo/releases/latest).

Installing it over a release, or the next release over it, upgrades in
place: the Windows installer, the \`.deb\` and the \`.rpm\` all know it
comes between the two. It keeps the same promise about your settings
and files as the release it leads to. With \`updates.channel = "nightly"\`, Home tells you when a
newer nightly is out. Report what goes wrong in an
[issue](https://github.com/$repo/issues), with what \`fenix --version\`
prints.

EOF
  if [[ -n $last && $last != "$commit" ]]; then
    echo "Changes since the last nightly: [\`${last:0:12}...${commit:0:12}\`](https://github.com/$repo/compare/$last...$commit)."
    echo
  fi
  echo "## Unreleased, from the changelog"
  echo
  if grep -q '[^[:space:]]' <<< "$changes"; then
    # Without the blank lines before its first heading.
    sed -e '/./,$!d' <<< "$changes"
  else
    echo "Nothing yet."
  fi
  echo
  echo "<!-- fenix-nightly version=$version commit=$commit -->"
}

command=${1-}
shift || true
case $command in
  version) [[ $# -ge 2 ]] || die "version <workspace version> <YYYYMMDD> [<last version>]"; version "$@" ;;
  needs-build) [[ $# -ge 1 ]] || die "needs-build <head> [<built>] [force]"; needs_build "$@" ;;
  record) [[ $# -eq 1 ]] || die "record <notes file>"; record "$1" ;;
  notes) [[ $# -ge 4 ]] || die "notes <version> <commit> <YYYY-MM-DD> <owner/repo> [<last commit>]"; notes "$@" ;;
  *) die "version, needs-build, record or notes" ;;
esac
