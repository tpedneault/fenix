#!/usr/bin/env bash
# Runs Fenix's remote-project checks against a real SSH server, in a
# container on this machine:
#   dev/remote-ssh/check.sh
# Needs Docker and an `ssh` client. Builds the helpers and the check
# program, starts the machine of ./Dockerfile on 127.0.0.1:$PORT (2222),
# signs in with a key made for the run, and takes it all down after.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
port=${PORT:-2222}
name=fenix-remote-ssh-check
work=$(mktemp -d)
cd "$root"

# A native program is given this system's own spelling of a path.
native() {
  if command -v cygpath > /dev/null; then cygpath -m "$1"; else echo "$1"; fi
}

cleanup() {
  docker rm -f "$name" > /dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

packaging/build-remote-helpers.sh target/remote-check
cargo build --release --locked -p fenix-ssh --bin fenix-ssh-check

docker build -q -t "$name" "$here" > /dev/null
docker rm -f "$name" > /dev/null 2>&1 || true
docker run -d --name "$name" -p "127.0.0.1:$port:22" "$name" > /dev/null
ssh-keygen -q -t ed25519 -N "" -f "$work/key"
MSYS_NO_PATHCONV=1 docker exec -i -u tom "$name" sh -c "cat > /home/tom/.ssh/authorized_keys && chmod 600 /home/tom/.ssh/authorized_keys" < "$work/key.pub"

# The server takes a moment to listen.
for _ in $(seq 1 50); do
  if MSYS_NO_PATHCONV=1 docker exec "$name" test -s /run/sshd.pid; then break; fi
  sleep 0.2
done

MSYS_NO_PATHCONV=1 target/release/fenix-ssh-check --helpers "$(native "$root/target/remote-check/remote")" "$@" -- \
  -p "$port" \
  -o "UserKnownHostsFile=$(native "$work/known_hosts")" -o StrictHostKeyChecking=accept-new \
  -o "IdentityFile=$(native "$work/key")" -o IdentitiesOnly=yes -o PreferredAuthentications=publickey \
  tom@127.0.0.1 /home/tom/project
