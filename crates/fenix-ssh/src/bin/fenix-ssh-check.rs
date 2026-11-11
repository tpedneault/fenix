//! The checks of docs/plans/remote-ssh.md that want a real machine: a
//! connection through the system's `ssh`, run and timed.
//!
//!     fenix-ssh-check [--helpers DIR] [--answer TEXT]… [--runs N] [-- SSH-OPTION…] HOST REMOTE-DIR
//!
//! REMOTE-DIR is a Git repository on HOST that the run may write a
//! scratch file in. `--answer` gives what `ssh` asks for, in order, for
//! a run with nobody at the keyboard; without it the question is put on
//! the console.
//!
//! Not shipped: a tool for whoever works on remote projects.

use std::collections::VecDeque;
use std::io::Write;
use std::process::{ExitCode, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fenix_ssh::askpass::Askpass;
use fenix_ssh::{connect, Error, Exec, FailureCode, Helpers, Host, Output, Session, Status, Target};

fn main() -> ExitCode {
    // This program is also what `ssh` runs to ask a question, and the
    // stand-in for a remote program.
    if let Some(code) = fenix_ssh::askpass::answer_if_asked() {
        std::process::exit(code);
    }
    if let Some(code) = fenix_ssh::bridge::stand_in_if_asked() {
        std::process::exit(code);
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("\nFAILED: {error}");
            ExitCode::FAILURE
        }
    }
}

type Outcome<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn run() -> Outcome {
    let mut helpers = std::path::PathBuf::from("target/helpers");
    let mut answers = VecDeque::new();
    let mut runs = 20usize;
    let mut options = Vec::new();
    let mut rest = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--helpers" => helpers = args.next().ok_or("--helpers DIR")?.into(),
            "--answer" => answers.push_back(args.next().ok_or("--answer TEXT")?),
            "--runs" => runs = args.next().ok_or("--runs N")?.parse()?,
            "--" => {
                let mut tail: Vec<String> = args.by_ref().collect();
                rest = tail.split_off(tail.len().saturating_sub(2));
                options = tail;
            }
            _ => rest.push(arg),
        }
    }
    let [host, dir] = rest.as_slice() else { return Err("usage: fenix-ssh-check [--helpers DIR] [--answer TEXT]… [-- SSH-OPTION…] HOST REMOTE-DIR".into()) };
    let target = Target { options, ..Target::new(host.clone()) };
    let helpers = Helpers { folder: helpers };

    let asked = Arc::new(AtomicUsize::new(0));
    let answers = Mutex::new(answers);
    let counting = asked.clone();
    let ask = Arc::new(move |prompt: &str| {
        counting.fetch_add(1, Ordering::SeqCst);
        if let Some(answer) = answers.lock().unwrap().pop_front() {
            println!("    ssh asks: {:?} -> answered from --answer", prompt.trim());
            return Some(answer);
        }
        eprint!("    ssh asks: {} ", prompt.trim());
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok()?;
        Some(line.trim_end_matches(['\r', '\n']).to_string()).filter(|answer| !answer.is_empty())
    });
    let asking = ask.clone();
    let askpass = Askpass::start(move |prompt| asking(prompt))?;

    println!("== connect");
    let began = Instant::now();
    let connected = connect(&target, &helpers, Some(&askpass))?;
    let hello = connected.session.hello().clone();
    println!("  first connect: {:?} (helper installed now: {}, questions asked: {})", began.elapsed(), connected.installed, asked.load(Ordering::SeqCst));
    println!("  helper: {} {}/{} as {} in {}", hello.version, hello.os, hello.arch, hello.user, hello.home);
    drop(connected);
    let before = asked.load(Ordering::SeqCst);
    let began = Instant::now();
    let connected = connect(&target, &helpers, Some(&askpass))?;
    println!("  next connect:  {:?} (installed now: {}, questions asked: {})", began.elapsed(), connected.installed, asked.load(Ordering::SeqCst) - before);
    drop(connected);

    // From here on, as Fenix will: a host that stays connected.
    let asking = ask.clone();
    let before = asked.load(Ordering::SeqCst);
    let machine = Host::connect("spike", target.clone(), helpers.clone(), move |prompt| asking(prompt))?;
    let session = machine.session()?;
    println!("  as a host: connected, questions asked: {}", asked.load(Ordering::SeqCst) - before);

    println!("== files");
    let entries = session.list(dir)?;
    println!("  {dir}: {} entries", entries.len());
    println!("  stat, median of {}: {:?}", runs * 5, median(runs * 5, || session.stat(dir).map(drop))?);
    println!("  list, median of {runs}: {:?}", median(runs, || session.list(dir).map(drop))?);

    let scratch = format!("{dir}/.fenix-spike.txt");
    let first = session.write(&scratch, b"one\n", None, true)?;
    sh(&session, &format!("chmod 754 '{scratch}'"))?;
    let (bytes, read) = session.read(&scratch)?;
    check(bytes == b"one\n", "what was written is what is read")?;
    let second = session.write(&scratch, b"two\n", Some(read.version()), false)?;
    check(second.mode == 0o754, &format!("a save keeps the file's mode ({:o})", second.mode))?;
    // Someone else changes it, a second later so the clock shows it too.
    sh(&session, &format!("sleep 1.1; echo theirs >> '{scratch}'"))?;
    match session.write(&scratch, b"mine\n", Some(second.version()), false) {
        Err(Error::Remote(failure)) if failure.code == FailureCode::Changed => println!("  ok: a save over a file changed there is refused ({})", failure.message),
        other => return Err(format!("a save over a changed file wasn't refused: {other:?}").into()),
    }
    check(session.read(&scratch)?.0 == b"two\ntheirs\n", "and the file is as they left it")?;
    let _ = first;

    for megabytes in [1usize, 16] {
        let big: Vec<u8> = (0..megabytes * 1024 * 1024).map(|i| (i * 31 % 251) as u8).collect();
        let began = Instant::now();
        session.write(&scratch, &big, None, false)?;
        let wrote = began.elapsed();
        let began = Instant::now();
        let back = session.read(&scratch)?.0;
        check(back == big, &format!("{megabytes} MiB: written in {wrote:?}, read in {:?}", began.elapsed()))?;
    }
    sh(&session, &format!("rm -f '{scratch}'"))?;
    check(!sh(&session, &format!("ls -a '{dir}'"))?.contains(".fenix-"), "nothing of Fenix's is left in the folder")?;

    println!("== programs");
    println!("  PATH there: {}", sh(&session, "echo $PATH")?.trim());
    println!("  git, rg, fd: {}", sh(&session, "command -v git rg fd; true")?.split_whitespace().collect::<Vec<_>>().join(" "));
    let git = || Exec { program: "git".into(), args: vec!["status".into(), "--porcelain".into()], cwd: Some(dir.clone()), ..Exec::default() };
    let status = session.output(git())?;
    check(status.code == Some(0), &format!("git status there: {} changed files", String::from_utf8_lossy(&status.stdout).lines().count()))?;
    println!("  git status over the session, median of {runs}:    {:?}", median(runs, || session.output(git()).map(drop))?);

    let bridge = &machine;
    let through = || -> Outcome<std::process::Output> { Ok(bridge.command("git", Some(dir))?.args(["status", "--porcelain"]).stdin(Stdio::null()).output()?) };
    let stood_in = through()?;
    check(stood_in.status.success() && stood_in.stdout == status.stdout, "the stand-in process gives git's own output and exit code")?;
    println!("  git status through the stand-in, median of {runs}: {:?}", median(runs, || through().map(drop))?);
    let plain = || -> Outcome {
        let mut ssh = target.command(&format!("cd '{dir}' && git status --porcelain"), Some(&askpass))?;
        check_quiet(ssh.stdin(Stdio::null()).output()?.status.success(), "plain ssh")
    };
    println!("  git status by one ssh each, median of 3:       {:?}", median(3, plain)?);
    let local = || -> Outcome { Ok(std::process::Command::new("git").arg("--version").output().map(drop)?) };
    println!("  (starting a local git, for scale, median of {runs}: {:?})", median(runs, local)?);

    let failing = bridge.command("sh", None)?.args(["-c", "echo no >&2; exit 7"]).stdin(Stdio::null()).output()?;
    check(failing.status.code() == Some(7) && failing.stderr == b"no\n", "the stand-in passes on stderr and a failing exit code")?;

    let mut variable = bridge.command("sh", None)?;
    fenix_ssh::bridge::env(&mut variable, "GIT_EDITOR", "true");
    let said = variable.args(["-c", "echo $GIT_EDITOR"]).stdin(Stdio::null()).output()?;
    check(said.stdout == b"true\n", "a variable set for the remote program reaches it")?;

    let megabyte: Vec<u8> = (0..1024 * 1024).map(|i| (i % 253) as u8).collect();
    let mut cat = bridge.command("cat", None)?.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
    let mut stdin = cat.stdin.take().expect("piped");
    let sending = megabyte.clone();
    let feeding = std::thread::spawn(move || stdin.write_all(&sending));
    let echoed = cat.wait_with_output()?;
    feeding.join().expect("the feeder runs")?;
    check(echoed.stdout == megabyte, "1 MiB through a remote program's stdin and back, by the stand-in")?;

    println!("== stopping");
    let sleeper = session.exec(Exec { program: "sh".into(), args: vec!["-c".into(), "sleep 311 & sleep 311".into()], ..Exec::default() })?;
    std::thread::sleep(Duration::from_millis(300));
    let began = Instant::now();
    sleeper.input().kill()?;
    loop {
        if let Output::Exit(code) = sleeper.next()? {
            println!("  ok: a killed program ends in {:?} (code {code:?})", began.elapsed());
            break;
        }
    }
    check(count(&session, "sleep 311")? == 0, "and what it started is gone with it")?;

    // A stand-in killed by its caller takes the remote program along.
    let mut orphan = bridge.command("sleep", None)?.arg("322").spawn()?;
    std::thread::sleep(Duration::from_millis(500));
    check(count(&session, "sleep 322")? == 1, "a program started through the stand-in runs there")?;
    orphan.kill()?;
    orphan.wait()?;
    std::thread::sleep(Duration::from_millis(500));
    check(count(&session, "sleep 322")? == 0, "killing the stand-in ends the remote program")?;

    // The connection goes: nothing stays behind.
    let doomed = connect(&target, &helpers, Some(&askpass))?.session;
    let _running = doomed.exec(Exec { program: "sleep".into(), args: vec!["333".into()], ..Exec::default() })?;
    std::thread::sleep(Duration::from_millis(500));
    check(count(&session, "sleep 333")? == 1, "a program runs on a second connection")?;
    let began = Instant::now();
    drop(doomed);
    std::thread::sleep(Duration::from_millis(700));
    check(count(&session, "sleep 333")? == 0, "ending the connection ends the helper's programs")?;
    check(count(&session, "fenix-remote --stdio")? == 1, "and only this connection's helper is left")?;
    let _ = began;

    println!("== as Fenix sees it: remote paths");
    let root = fenix_vfs::remote("spike", dir);
    check(fenix_vfs::home("spike")? == fenix_vfs::remote("spike", &hello.home), "the machine's home is a remote path")?;
    let listed = fenix_vfs::list(&root)?;
    check(listed.iter().any(|entry| entry.name == ".git" && entry.is_dir()), &format!("a remote folder lists through its path ({} entries)", listed.len()))?;
    let tracked = fenix_vfs::run_there(&root, fenix_vfs::Run { program: "git", args: &["ls-files", "-z"], ..Default::default() }).expect("a remote folder")?;
    let tracked: Vec<String> = String::from_utf8_lossy(&tracked.stdout).split('\0').filter(|name| !name.is_empty()).map(str::to_string).collect();
    check(!tracked.is_empty(), &format!("the machine's git lists the project's files ({})", tracked.len()))?;
    let first = fenix_vfs::join(&root, &tracked[0]);
    let (bytes, meta) = fenix_vfs::read_with_meta(&first)?;
    check(meta.is_file() && meta.len == bytes.len() as u64, &format!("{} reads through its path ({} bytes)", fenix_vfs::display(&first), bytes.len()))?;
    // Names this machine would misread, made there and found again.
    let odd = fenix_vfs::child(&root, ".fenix-check a:b");
    fenix_vfs::write(&odd, b"odd", fenix_vfs::Over::Nothing)?;
    check(fenix_vfs::list(&root)?.iter().any(|entry| entry.name == ".fenix-check a:b") && fenix_vfs::read(&odd)? == b"odd", "a name with a colon and a space is a file there")?;
    let refused = fenix_vfs::write(&odd, b"again", fenix_vfs::Over::Version(fenix_vfs::Version { len: 99, mtime: 0 })).unwrap_err();
    check(fenix_vfs::changed(&refused), "a save held against another version is refused as changed")?;
    // Up to the machine and back down.
    let here = std::env::temp_dir().join(format!("fenix-ssh-check-{}", std::process::id()));
    std::fs::create_dir_all(here.join("up/deep"))?;
    std::fs::write(here.join("up/deep/file.txt"), b"went up")?;
    let up = fenix_vfs::child(&root, ".fenix-check-up");
    fenix_vfs::copy(&here.join("up"), &up, &|| true)?;
    fenix_vfs::copy(&up, &here.join("down"), &|| true)?;
    check(std::fs::read(here.join("down/deep/file.txt"))? == b"went up", "a folder copies to the machine and back")?;
    fenix_vfs::remove(&up, true)?;
    fenix_vfs::remove(&odd, false)?;
    std::fs::remove_dir_all(&here)?;
    check(!sh(&session, &format!("ls -a '{dir}'"))?.contains(".fenix-check"), "and nothing of the check is left there")?;

    println!("== terminals, search and Git (phase 3)");
    // The user's own shell there, as a login shell, in a pty of its own.
    let began = Instant::now();
    let shell = fenix_vfs::Shell { rows: 24, cols: 80, ..fenix_vfs::Shell::default() };
    let mut terminal = fenix_vfs::terminal(&root, shell).expect("a remote folder")?;
    let mut said = String::new();
    // What's typed is echoed: the marks looked for are worked out there.
    terminal.writer.write_all(b"tty; echo TERM=$TERM; stty size; echo PATH=$PATH; pwd; echo DONE$((40+2))\n")?;
    read_until(&mut terminal.reader, &mut said, "DONE42")?;
    println!("  a shell answered in {:?}", began.elapsed());
    check(said.contains("/dev/pts/"), "the shell has a terminal of its own")?;
    check(said.contains("TERM=xterm-256color") && said.contains("24 80"), "of the kind and size asked")?;
    check(said.contains(&format!("{}/bin", hello.home)), "with the login shell's PATH")?;
    check(said.lines().any(|line| line.trim() == dir.as_str()), "in the project's folder")?;
    terminal.control.resize(40, 120)?;
    said.clear();
    terminal.writer.write_all(b"stty size; echo SIZED$((1+1))\n")?;
    read_until(&mut terminal.reader, &mut said, "SIZED2")?;
    check(said.contains("40 120"), "resized")?;
    terminal.writer.write_all(b"sleep 344 & echo STARTED$((2+2))\n")?;
    said.clear();
    read_until(&mut terminal.reader, &mut said, "STARTED4")?;
    check(count(&session, "sleep 344")? == 1, "a job runs in it")?;
    terminal.control.kill();
    let mut rest = String::new();
    let _ = read_until(&mut terminal.reader, &mut rest, "\u{0}never");
    check(terminal.control.exited().is_some(), "a terminal that's closed ends")?;
    std::thread::sleep(Duration::from_millis(300));
    check(count(&session, "sleep 344")? == 0, "and what was started in it ends with it")?;
    let mut exits = fenix_vfs::terminal(&root, fenix_vfs::Shell { program: Some("sh"), rows: 24, cols: 80, ..fenix_vfs::Shell::default() }).expect("a remote folder")?;
    exits.writer.write_all(b"exit 3\n")?;
    let _ = read_until(&mut exits.reader, &mut String::new(), "\u{0}never");
    check(exits.control.exited() == Some(Some(3)), "a shell's exit code comes back")?;

    // Search with the helper's own engine: no `rg` there.
    let paths = fenix_vfs::SearchPaths { folders: vec![root.clone()], ..Default::default() };
    let mut found = Vec::new();
    let began = Instant::now();
    let searched = fenix_vfs::search(&paths, fenix_vfs::Search { pattern: "main", ..Default::default() }, &mut |file| {
        if let Some((path, lines)) = file {
            found.push((path, lines));
        }
        true
    })
    .expect("a remote folder")?;
    check(!found.is_empty(), &format!("a search there finds {} files in {:?} ({} searched)", found.len(), began.elapsed(), searched.files_searched))?;
    check(found.iter().all(|(path, _)| fenix_vfs::host_of(path).as_deref() == Some("spike")), "as paths on the machine")?;
    check(!found.iter().any(|(path, _)| fenix_vfs::display(path).contains("/.git/")), "and nothing of .git")?;

    // Git, as fenix-git runs it: the machine's own, with no prompt.
    let git = |args: &[&str]| fenix_vfs::run_there(&root, fenix_vfs::Run { program: "git", args, env: &[("GIT_TERMINAL_PROMPT", "0"), ("GIT_EDITOR", "true")], ..Default::default() }).expect("a remote folder");
    let scratch = fenix_vfs::child(&root, ".fenix-check-git.txt");
    fenix_vfs::write(&scratch, b"new\n", fenix_vfs::Over::Nothing)?;
    let status = git(&["status", "--porcelain=v2", "--branch"])?;
    let status = String::from_utf8_lossy(&status.stdout).into_owned();
    check(status.contains("# branch.head") && status.contains(".fenix-check-git.txt"), "git status there sees the branch and a new file")?;
    let patch = git(&["diff", "--no-index", "--no-color", "/dev/null", ".fenix-check-git.txt"])?;
    check(String::from_utf8_lossy(&patch.stdout).contains("+new"), "git diff there")?;
    let applied = fenix_vfs::run_there(&root, fenix_vfs::Run { program: "git", args: &["apply", "--cached", "-"], stdin: Some(&patch.stdout), ..Default::default() }).expect("a remote folder")?;
    check(applied.success(), "a patch sent to git apply's stdin is staged")?;
    let staged = git(&["diff", "--cached", "--name-only"])?;
    check(String::from_utf8_lossy(&staged.stdout).contains(".fenix-check-git.txt"), "and shows as staged")?;
    git(&["reset", "-q", "--", ".fenix-check-git.txt"])?;
    fenix_vfs::remove(&scratch, false)?;
    let clean = git(&["status", "--porcelain"])?;
    check(!String::from_utf8_lossy(&clean.stdout).contains(".fenix-check"), "and the repository is left as it was")?;

    println!("== a connection that drops");
    let began = Instant::now();
    session.sever();
    while machine.session().is_ok() {
        std::thread::sleep(Duration::from_millis(10));
    }
    check(session.stat(dir).is_err(), &format!("the loss is noticed in {:?}, and a request fails instead of waiting", began.elapsed()))?;
    let before = asked.load(Ordering::SeqCst);
    while machine.status() != Status::Connected || machine.session().is_err() {
        if let Status::Down(why) = machine.status() {
            return Err(format!("it didn't come back: {why}").into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    println!("  ok: connected again by itself after {:?}, with {} more questions", began.elapsed(), asked.load(Ordering::SeqCst) - before);
    check(!machine.session()?.list(dir)?.is_empty(), "and the files are there again")?;
    machine.close();

    println!("\nall passed");
    Ok(())
}

/// Reads a terminal's output into `said` until it holds `want`, or ends.
fn read_until(reader: &mut dyn std::io::Read, said: &mut String, want: &str) -> Outcome {
    let mut buffer = [0u8; 4096];
    let until = Instant::now() + Duration::from_secs(20);
    while !said.contains(want) {
        if Instant::now() > until {
            return Err(format!("no {want:?} from the terminal: {said:?}").into());
        }
        match reader.read(&mut buffer)? {
            0 => return Err(format!("the terminal ended before {want:?}: {said:?}").into()),
            n => said.push_str(&String::from_utf8_lossy(&buffer[..n])),
        }
    }
    Ok(())
}

fn sh(session: &Session, script: &str) -> Outcome<String> {
    let finished = session.output(Exec { program: "sh".into(), args: vec!["-c".into(), script.into()], ..Exec::default() })?;
    if finished.code != Some(0) {
        return Err(format!("`{script}` failed: {}", String::from_utf8_lossy(&finished.stderr)).into());
    }
    Ok(String::from_utf8_lossy(&finished.stdout).into_owned())
}

/// How many processes there have `command` as their whole command line.
fn count(session: &Session, command: &str) -> Outcome<usize> {
    Ok(sh(session, "ps -eo args")?.lines().filter(|line| line.trim() == command || line.trim().ends_with(&format!("/{command}"))).count())
}

fn check(passed: bool, what: &str) -> Outcome {
    if passed {
        println!("  ok: {what}");
        Ok(())
    } else {
        Err(format!("not so: {what}").into())
    }
}

fn check_quiet(passed: bool, what: &str) -> Outcome {
    if passed {
        Ok(())
    } else {
        Err(format!("{what} failed").into())
    }
}

fn median<E: Into<Box<dyn std::error::Error>>>(runs: usize, mut once: impl FnMut() -> Result<(), E>) -> Outcome<Duration> {
    let mut times = Vec::with_capacity(runs);
    for _ in 0..runs {
        let began = Instant::now();
        once().map_err(Into::into)?;
        times.push(began.elapsed());
    }
    times.sort();
    Ok(times[times.len() / 2])
}
