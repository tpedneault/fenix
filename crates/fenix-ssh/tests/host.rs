//! A `Host` against `fake-ssh`: an `ssh` whose remote machine is this
//! one, with the real helper serving in it.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fenix_ssh::{Helpers, Host, Status, Target};
use fenix_vfs::{Over, Remote};

struct Machine {
    state: tempfile::TempDir,
    files: tempfile::TempDir,
}

impl Machine {
    fn new() -> Machine {
        Machine { state: tempfile::tempdir().unwrap(), files: tempfile::tempdir().unwrap() }
    }

    fn target(&self) -> Target {
        Target { host: "fake".into(), options: vec!["-o".into(), format!("FakeState={}", self.state.path().display())], ssh: env!("CARGO_BIN_EXE_fake-ssh").into() }
    }

    fn helpers(&self) -> Helpers {
        let folder = self.state.path().join("helpers");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("fenix-remote-linux-x86_64"), b"a helper, for the count").unwrap();
        Helpers { folder }
    }

    fn connect(&self, name: &str) -> Arc<Host> {
        Host::connect(name, self.target(), self.helpers(), |_| None).unwrap()
    }

    /// A path on the "remote machine", as the helper there takes it.
    fn path(&self, name: &str) -> String {
        self.files.path().join(name).to_string_lossy().into_owned()
    }

    fn set(&self, name: &str, text: &str) {
        std::fs::write(self.state.path().join(name), text).unwrap();
    }

    fn runs(&self) -> u32 {
        std::fs::read_to_string(self.state.path().join("runs")).unwrap().trim().parse().unwrap()
    }
}

fn until(what: &str, mut is: impl FnMut() -> bool) {
    let began = Instant::now();
    while !is() {
        assert!(began.elapsed() < Duration::from_secs(20), "waited too long for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn statuses(host: &Host) -> Arc<Mutex<Vec<Status>>> {
    let seen: Arc<Mutex<Vec<Status>>> = Arc::default();
    let seeing = seen.clone();
    host.on_status(move |status| seeing.lock().unwrap().push(status.clone()));
    seen
}

#[test]
fn a_machine_without_the_helper_gets_it_then_serves_files() {
    let machine = Machine::new();
    machine.set("no-helper", "");
    let host = machine.connect("fake-install");
    assert_eq!(machine.runs(), 2, "one run to install, one to start");
    assert_eq!(std::fs::read(machine.state.path().join("installed")).unwrap(), b"a helper, for the count");
    assert_eq!(host.status(), Status::Connected);
    assert!(fenix_vfs::mounted().contains(&"fake-install".to_string()));

    let file = machine.path("a.txt");
    let written = host.write(&file, b"one", Over::Nothing).unwrap();
    assert_eq!(written.len, 3);
    // A remote path has `/` in it, which this machine's may not.
    #[cfg(unix)]
    assert_eq!(written.name, "a.txt");
    let (bytes, read) = host.read(&file).unwrap();
    assert_eq!(bytes, b"one");
    std::fs::write(Path::new(&file), b"someone else's").unwrap();
    assert!(fenix_vfs::changed(&host.write(&file, b"two", Over::Version(read.version)).unwrap_err()));
    assert_eq!(host.write(&file, b"two", Over::Nothing).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(host.stat(&machine.path("nope")).unwrap_err().kind(), std::io::ErrorKind::NotFound);
    assert_eq!(host.list(&machine.path("")).unwrap().len(), 1);

    // The same through a remote path, where this machine's paths are
    // the remote's.
    #[cfg(unix)]
    assert_eq!(fenix_vfs::read(&fenix_vfs::remote("fake-install", &file)).unwrap(), b"someone else's");

    host.close();
    assert!(!fenix_vfs::mounted().contains(&"fake-install".to_string()));
    assert_eq!(host.stat(&file).unwrap_err().kind(), std::io::ErrorKind::NotConnected);
}

#[test]
fn a_lost_connection_comes_back_by_itself() {
    let machine = Machine::new();
    let host = machine.connect("fake-drop");
    let seen = statuses(&host);
    let file = machine.path("a.txt");
    host.write(&file, b"before", Over::Anything).unwrap();

    host.session().unwrap().sever();
    until("the loss to be noticed", || host.session().is_err());
    assert_eq!(host.stat(&file).unwrap_err().kind(), std::io::ErrorKind::NotConnected);
    until("the connection to come back", || host.status() == Status::Connected && host.session().is_ok());
    assert_eq!(host.read(&file).unwrap().0, b"before");
    let seen = seen.lock().unwrap().clone();
    assert!(matches!(seen.as_slice(), [Status::Reconnecting { attempt: 1, .. }, Status::Connected]), "{seen:?}");
    host.close();
}

#[test]
fn an_unreachable_machine_is_tried_again_and_a_refusal_waits_to_be_asked() {
    let machine = Machine::new();
    let host = machine.connect("fake-refuse");

    // Unreachable: tried again without anyone asking.
    machine.set("refuse", "ssh: connect to host fake port 22: Connection timed out");
    host.session().unwrap().sever();
    until("a second try", || matches!(host.status(), Status::Reconnecting { attempt, .. } if attempt >= 2));
    std::fs::remove_file(machine.state.path().join("refuse")).unwrap();
    until("the connection to come back", || host.status() == Status::Connected);

    // Refused: no more tries until someone says so.
    machine.set("refuse", "tom@fake: Permission denied (publickey,password).");
    host.session().unwrap().sever();
    until("the refusal", || matches!(host.status(), Status::Down(why) if why.contains("Permission denied")));
    let runs = machine.runs();
    std::thread::sleep(Duration::from_millis(900));
    assert_eq!(machine.runs(), runs, "a refused sign-in isn't hammered");
    std::fs::remove_file(machine.state.path().join("refuse")).unwrap();
    host.reconnect();
    until("the connection to come back", || host.status() == Status::Connected);
    host.close();
}
