# A machine for remote projects

A Debian container with an SSH server, for trying and testing remote
projects (`docs/plans/remote-ssh.md`) without a second computer.

```sh
dev/remote-ssh/check.sh
```

builds the helpers (`fenix-remote`, for x86_64 and aarch64 Linux) and the
check program (`fenix-ssh-check`), starts the container, and runs every
check against it through the system's `ssh`: connecting, installing the
helper, files, programs, stopping them, terminals, search and Git, and a
connection that drops. CI runs the same on Linux. From Windows, run it
in Git Bash.

To keep the machine up and connect by hand:

```sh
docker build -t fenix-remote-ssh dev/remote-ssh
docker run -d --name fenix-remote-ssh -p 127.0.0.1:2222:22 fenix-remote-ssh
ssh -p 2222 tom@127.0.0.1        # password: spike-pw
```

`fenix-ssh-check --answer TEXT` answers what `ssh` asks, in order, for a
sign-in with a password or a key's passphrase.

The app's own test against a machine, not run by CI: a remote project
there gets its terminal, Git and search. With the machine up as above,
your key in `/home/tom/.ssh/authorized_keys`, and the helpers built
(`packaging/build-remote-helpers.sh target/remote-check`):

```sh
FENIX_REMOTE_TEST="tom@127.0.0.1 /home/tom/project" \
FENIX_REMOTE_TEST_SSH="-p 2222 -o IdentityFile=$HOME/.ssh/id_ed25519" \
FENIX_REMOTE_TEST_HELPERS="$PWD/target/remote-check/remote" \
cargo test -p fenix-gui -- --ignored a_real_machines_project
```

It is a real `sshd`, but on this machine: there is no network between
the two, so no latency, and nothing that drops by itself.
