# A machine for remote projects

A Debian container with an SSH server, for trying and testing remote
projects (`docs/plans/remote-ssh.md`) without a second computer.

```sh
dev/remote-ssh/check.sh
```

builds the helpers (`fenix-remote`, for x86_64 and aarch64 Linux) and the
check program (`fenix-ssh-check`), starts the container, and runs every
check against it through the system's `ssh`: connecting, installing the
helper, files, programs, stopping them, and a connection that drops. CI
runs the same on Linux. From Windows, run it in Git Bash.

To keep the machine up and connect by hand:

```sh
docker build -t fenix-remote-ssh dev/remote-ssh
docker run -d --name fenix-remote-ssh -p 127.0.0.1:2222:22 fenix-remote-ssh
ssh -p 2222 tom@127.0.0.1        # password: spike-pw
```

`fenix-ssh-check --answer TEXT` answers what `ssh` asks, in order, for a
sign-in with a password or a key's passphrase.

It is a real `sshd`, but on this machine: there is no network between
the two, so no latency, and nothing that drops by itself.
