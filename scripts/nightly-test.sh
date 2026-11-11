#!/usr/bin/env bash
# Tests for scripts/nightly.sh, what the nightly workflow decides with:
#   bash scripts/nightly-test.sh
# Where dpkg (or rpm) is installed, it also checks the packages' versions
# sort the way the script's comment says.
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd)
n="$here/nightly.sh"
failed=0
pass() { echo "ok   $1"; }
fail() { echo "FAIL $1"; failed=1; }
same() { # <name> <expected> <actual>
  if [[ $2 == "$3" ]]; then pass "$1"; else fail "$1: expected '$2', got '$3'"; fi
}

head=0123456789abcdef0123456789abcdef01234567
other=fedcba9876543210fedcba9876543210fedcba98

# The version.
same "the next minor, dated" 1.6.0-nightly.20261112 "$(bash "$n" version 1.5.2 20261112)"
same "after a minor" 1.7.0-nightly.20261112 "$(bash "$n" version 1.6.0 20261112)"
same "two-digit minor" 2.11.0-nightly.20270101 "$(bash "$n" version 2.10.3 20270101)"
same "another day's last" 1.6.0-nightly.20261113 "$(bash "$n" version 1.5.2 20261113 1.6.0-nightly.20261112)"
same "the same day again" 1.6.0-nightly.20261112.2 "$(bash "$n" version 1.5.2 20261112 1.6.0-nightly.20261112)"
same "and again" 1.6.0-nightly.20261112.3 "$(bash "$n" version 1.5.2 20261112 1.6.0-nightly.20261112.2)"
same "a day's count isn't another day's" 1.6.0-nightly.20261113 "$(bash "$n" version 1.5.2 20261113 1.6.0-nightly.20261112.4)"
same "last minor's nightly, same day" 1.7.0-nightly.20261112 "$(bash "$n" version 1.6.0 20261112 1.6.0-nightly.20261112)"
if bash "$n" version 1.6.0-rc.1 20261112 > /dev/null 2>&1; then fail "a pre-release workspace version is refused"; else pass "a pre-release workspace version is refused"; fi
if bash "$n" version 1.5.2 2026-11-12 > /dev/null 2>&1; then fail "a date with dashes is refused"; else pass "a date with dashes is refused"; fi

# Anything new?
same "no nightly yet" true "$(bash "$n" needs-build $head "" 2> /dev/null)"
same "master unchanged" false "$(bash "$n" needs-build $head $head 2> /dev/null)"
same "master moved" true "$(bash "$n" needs-build $head $other 2> /dev/null)"
same "unchanged but forced" true "$(bash "$n" needs-build $head $head true 2> /dev/null)"
same "not forced" false "$(bash "$n" needs-build $head $head false 2> /dev/null)"
if bash "$n" needs-build main $head > /dev/null 2>&1; then fail "a branch name isn't a commit"; else pass "a branch name isn't a commit"; fi

# The notes, and the record in them.
changelog=$(mktemp)
notes=$(mktemp)
trap 'rm -f "$changelog" "$notes"' EXIT
cat > "$changelog" <<'EOF'
# Changelog

## [Unreleased]

### Added

- A nightly build.

## [1.5.2] - 2026-11-11

### Added

- Soft wrap.
EOF
bash "$n" notes 1.6.0-nightly.20261112 $head 2026-11-12 tpedneault/fenix $other < "$changelog" > "$notes"
grep -q '^### Added$' "$notes" && pass "the Unreleased section's headings" || fail "the Unreleased section's headings"
grep -q '^- A nightly build\.$' "$notes" && pass "the Unreleased entries" || fail "the Unreleased entries"
grep -q 'Soft wrap' "$notes" && fail "a release's entries are left out" || pass "a release's entries are left out"
grep -q "compare/$other\.\.\.$head" "$notes" && pass "the changes since the last nightly" || fail "the compare link"
grep -q "commit/$head" "$notes" && pass "the commit" || fail "the commit"
same "the record reads back" "1.6.0-nightly.20261112 $head" "$(bash "$n" record "$notes")"
bash "$n" notes 1.6.0-nightly.20261112 $head 2026-11-12 tpedneault/fenix < "$changelog" | grep -q compare && fail "no compare link for the first" || pass "no compare link for the first"
printf '# Changelog\n\n## [Unreleased]\n\n## [1.5.2]\n- x\n' | bash "$n" notes 1.6.0-nightly.20261112 $head 2026-11-12 tpedneault/fenix | grep -q '^Nothing yet\.$' && pass "an empty Unreleased section" || fail "an empty Unreleased section"
echo "no record here" > "$notes"
same "notes without a record" "" "$(bash "$n" record "$notes")"

# The packages' versions sort as SemVer's do: last night's, tonight's,
# tonight's second, the release. nfpm writes 1.6.0-nightly.X as
# 1.6.0~nightly.X for both.
order=(1.5.2 1.6.0~nightly.20261111 1.6.0~nightly.20261112 1.6.0~nightly.20261112.2 1.6.0~nightly.20261113 1.6.0 1.6.1 1.7.0~nightly.20261201)
if command -v dpkg > /dev/null; then
  for ((i = 0; i + 1 < ${#order[@]}; i++)); do
    a=${order[i]} b=${order[i + 1]}
    dpkg --compare-versions "$a" lt "$b" && pass "dpkg: $a < $b" || fail "dpkg: $a < $b"
  done
  # What a hyphen would have meant: revision rc.1 of 1.6.0, after it.
  dpkg --compare-versions 1.6.0-nightly.1 gt 1.6.0 && pass "dpkg: why it's ~, not -" || fail "dpkg: why it's ~"
else
  echo "skip dpkg: not installed"
fi
if command -v rpm > /dev/null; then
  for ((i = 0; i + 1 < ${#order[@]}; i++)); do
    a=${order[i]} b=${order[i + 1]}
    [[ $(rpm --eval "%{lua: print(rpm.vercmp('$a', '$b'))}") == -1 ]] && pass "rpm: $a < $b" || fail "rpm: $a < $b"
  done
else
  echo "skip rpm: not installed"
fi

exit $failed
