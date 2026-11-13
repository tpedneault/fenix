//! `fake-test-runner` run through `TaskRunner`, as the test explorer runs
//! a step: its script, and a stop that ends the whole tree -- the child it
//! left holding the pipes too -- on Windows (the job object) as on Unix
//! (the process group). Every wait is bounded.

use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use fenix_tasks::fake_runner::gone_within;
use fenix_tasks::{OutputLine, TaskEvent, TaskOutcome, TaskRunner};

fn runner(dir: &std::path::Path, args: &[&str]) -> (TaskRunner, mpsc::Receiver<TaskEvent>) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_fake-test-runner"));
    command.args(args).current_dir(dir);
    let (tx, rx) = mpsc::channel();
    (TaskRunner::spawn_command(command, move |e| tx.send(e).is_ok()).unwrap(), rx)
}

/// Its output until `done` says so, or it finishes; at most 30 s.
fn until(rx: &mpsc::Receiver<TaskEvent>, done: impl Fn(&[OutputLine]) -> bool) -> (Vec<OutputLine>, Option<TaskOutcome>) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut lines = Vec::new();
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("the runner within 30 s") {
            TaskEvent::Output(batch) => {
                lines.extend(batch);
                if done(&lines) {
                    return (lines, None);
                }
            }
            TaskEvent::Finished(outcome) => return (lines, Some(outcome)),
        }
    }
}

#[test]
fn its_script_is_what_it_prints_and_how_it_ends() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.fake"), "# test one\n#! out PASS a.fake::one 3ms\n#! err on stderr\n#! exit 3\n").unwrap();
    let (_runner, rx) = runner(dir.path(), &["a.fake", "a.fake::one"]);
    let (lines, outcome) = until(&rx, |_| false);
    let text: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(text, ["PASS a.fake::one 3ms", "on stderr"], "the file's script, once");
    assert_eq!(outcome, Some(TaskOutcome::Exited { success: false, code: Some(3) }));
}

/// A stop kills what the runner started too: cargo's test binary goes
/// with cargo (plan §8.2, §14).
#[test]
fn a_stop_ends_the_child_it_left_holding_the_pipes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("fake-tree.txt"), "child\nwait\n").unwrap();
    let (runner, rx) = runner(dir.path(), &["--tree"]);
    let (lines, outcome) = until(&rx, |lines| lines.iter().any(|l| l.text.starts_with("CHILD ")));
    assert_eq!(outcome, None, "{lines:?}");
    let pid: u32 = lines.iter().find_map(|l| l.text.strip_prefix("CHILD ")).unwrap().trim().parse().unwrap();
    assert!(fenix_tasks::fake_runner::alive(pid), "the child runs");
    runner.cancel();
    let (_, outcome) = until(&rx, |_| false);
    assert_eq!(outcome, Some(TaskOutcome::Cancelled));
    assert!(gone_within(pid, Duration::from_secs(10)), "the child outlived its step");
}
