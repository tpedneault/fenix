# A machine to open a remote project on: Debian with an SSH server, Git,
# a user `tom` (password `spike-pw`), and a small Git repository in
# /home/tom/project. Its shell greets, as real ones do, and its login
# profile puts ~/bin on PATH. See README.md.
FROM debian:stable-slim
RUN apt-get update && apt-get install -y --no-install-recommends openssh-server git procps && rm -rf /var/lib/apt/lists/* && mkdir -p /run/sshd
RUN useradd -m -s /bin/bash tom && echo 'tom:spike-pw' | chpasswd \
 && printf 'PasswordAuthentication yes\nPubkeyAuthentication yes\n' > /etc/ssh/sshd_config.d/fenix.conf
USER tom
RUN mkdir -p ~/.ssh ~/bin ~/project/src && chmod 700 ~/.ssh \
 && sed -i '1i echo "Welcome to the test machine. Have a lot of fun."' ~/.bashrc \
 && echo 'export PATH="$HOME/bin:$PATH"' >> ~/.profile \
 && cd ~/project && git init -q -b main && git config user.email tom@example.com && git config user.name Tom \
 && printf 'fn main() {}\n' > src/main.rs && printf '# project\n' > README.md && git add . && git commit -q -m "A project"
USER root
EXPOSE 22
CMD ["/usr/sbin/sshd", "-D", "-e"]
