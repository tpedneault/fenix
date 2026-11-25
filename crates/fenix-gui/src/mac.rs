//! What macOS wants of an application that isn't part of any window: the
//! environment a Finder launch lacks, files Finder opens with Fenix, and a
//! Quit from the menu, the Dock or a log-out that asks about unsaved work
//! first. Nothing here runs on another system.

use std::collections::HashMap;
use std::ffi::{c_char, CStr, OsString};
use std::io::Read;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use objc2::ffi::class_addMethod;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{class, msg_send, sel};
use winit::event_loop::EventLoopProxy;

use crate::app::FenixUserEvent;

/// Before anything else in `main`: this program's own path, resolved, and
/// what a launch from Finder or the Dock leaves out.
pub fn prepare() {
    restart_at_real_path();
    // A C locale makes shells and Git print UTF-8 names as escapes; a GUI
    // launch has none at all.
    if ["LANG", "LC_ALL", "LC_CTYPE"].iter().all(|v| std::env::var_os(v).is_none()) {
        std::env::set_var("LC_CTYPE", "UTF-8");
    }
    if !launched_by_launchd() {
        return;
    }
    // Launched from Finder, the folder is `/`: nobody's project.
    if std::env::current_dir().is_ok_and(|d| d == std::path::Path::new("/")) {
        if let Some(home) = std::env::var_os("HOME") {
            let _ = std::env::set_current_dir(home);
        }
    }
    take_login_environment();
}

/// `fenix` on PATH is a link into `Fenix.app`, and macOS gives a program
/// the path it was started by, link and all: what ships beside Fenix (the
/// PDF companion, the remote helpers) wouldn't be beside it. Started by a
/// link, Fenix becomes the program it names.
fn restart_at_real_path() {
    use std::os::unix::process::CommandExt;
    let Ok(exe) = std::env::current_exe() else { return };
    if !exe.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
        return;
    }
    let Ok(real) = exe.canonicalize() else { return };
    // Returns only when it couldn't; then this one goes on.
    let _ = Command::new(real).args(std::env::args_os().skip(1)).exec();
}

/// Whether launchd started this (Finder, the Dock, Spotlight, `open`)
/// rather than a shell.
fn launched_by_launchd() -> bool {
    // SAFETY: getppid has no preconditions and can't fail.
    let parent = unsafe { libc::getppid() };
    parent == 1 && std::env::var_os("TERM").is_none()
}

/// The environment a login shell would have: launchd's PATH is the
/// system's folders alone, without Homebrew's, cargo's, or anything a
/// profile adds, so no language server or tool would be found. A shell
/// that takes too long is left behind and Fenix starts as it was.
fn take_login_environment() {
    let Some(text) = login_environment(Duration::from_secs(5)) else { return };
    for (key, value) in parse_environment(&text) {
        let name = key.to_string_lossy();
        let own = matches!(&*name, "PWD" | "OLDPWD" | "SHLVL" | "_" | "TERM") || name.starts_with("FENIX_");
        // What launchd set stands, but for PATH.
        if !own && (name == "PATH" || std::env::var_os(&key).is_none()) {
            std::env::set_var(key, value);
        }
    }
}

/// Between these the shell's environment is printed: a profile may print
/// things of its own.
const MARK: &str = "__FENIX_ENVIRONMENT__";

fn login_environment(limit: Duration) -> Option<Vec<u8>> {
    use std::os::unix::process::CommandExt;
    let shell = std::env::var_os("SHELL").filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
    // One command line every shell reads the same way: zsh, bash, fish.
    let script = format!("/usr/bin/printf %s {MARK}; /usr/bin/env -0; /usr/bin/printf %s {MARK}");
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c", &script])
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // A group of its own: an interactive shell takes the terminal's
        // otherwise, and one that hangs is ended with what it started.
        .process_group(0)
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut all, mut chunk) = (Vec::new(), [0u8; 8192]);
        // To the second mark, not the end: something the profile started
        // in the background can hold the pipe open.
        while let Ok(n @ 1..) = out.read(&mut chunk) {
            all.extend_from_slice(&chunk[..n]);
            if between_marks(&all).is_some() {
                break;
            }
        }
        let _ = send.send(all);
    });
    let all = receive.recv_timeout(limit).ok();
    if all.is_none() {
        // SAFETY: a signal to the group of a child not yet waited for.
        unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
    }
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    between_marks(&all?).map(<[u8]>::to_vec)
}

fn between_marks(all: &[u8]) -> Option<&[u8]> {
    let find = |from: usize| all[from..].windows(MARK.len()).position(|w| w == MARK.as_bytes()).map(|at| at + from);
    let start = find(0)? + MARK.len();
    Some(&all[start..find(start)?])
}

fn parse_environment(text: &[u8]) -> HashMap<OsString, OsString> {
    text.split(|&b| b == 0)
        .filter_map(|entry| {
            let at = entry.iter().position(|&b| b == b'=').filter(|&at| at > 0)?;
            Some((OsString::from_vec(entry[..at].to_vec()), OsString::from_vec(entry[at + 1..].to_vec())))
        })
        .collect()
}

/// Where the delegate's methods send what they're told.
static PROXY: OnceLock<EventLoopProxy<FenixUserEvent>> = OnceLock::new();
/// Whether there's work that quitting would lose, as the editor last said
/// (`set_unsaved`): AppKit asks for an answer on the spot.
static UNSAVED: AtomicBool = AtomicBool::new(false);

/// Says whether quitting now would lose anything.
pub fn set_unsaved(unsaved: bool) {
    UNSAVED.store(unsaved, Ordering::Relaxed);
}

/// Once winit's event loop is built: Fenix's answers added to the
/// application delegate it made. Winit keeps that delegate its own (its
/// event loop panics with another's) and answers neither of these.
pub fn install(proxy: EventLoopProxy<FenixUserEvent>) {
    let _ = PROXY.set(proxy);
    // SAFETY: on the main thread, after `sharedApplication` and its
    // delegate exist. The two methods' types are the ones AppKit declares
    // for their selectors, and are what the encodings say.
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let delegate: *mut AnyObject = msg_send![app, delegate];
        if delegate.is_null() {
            return;
        }
        let class: *mut AnyClass = (*delegate).class() as *const AnyClass as *mut AnyClass;
        let should_quit: extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize = should_terminate;
        class_addMethod(class, sel!(applicationShouldTerminate:), std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> usize, Imp>(should_quit), c"Q@:@".as_ptr());
        let open: extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject) = open_urls;
        class_addMethod(class, sel!(application:openURLs:), std::mem::transmute::<extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject), Imp>(open), c"v@:@@".as_ptr());
        // AppKit notes what a delegate answers when it's set.
        let _: () = msg_send![app, setDelegate: delegate];
        // Holding a key repeats it (`j`), rather than offering accents.
        let key: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: c"ApplePressAndHoldEnabled".as_ptr()];
        let defaults: *mut AnyObject = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let no: *mut AnyObject = msg_send![class!(NSNumber), numberWithBool: false];
        let values: *mut AnyObject = msg_send![class!(NSDictionary), dictionaryWithObject: no, forKey: key];
        // Registered, not set: your own `defaults write` is kept.
        let _: () = msg_send![defaults, registerDefaults: values];
    }
}

/// `NSTerminateNow` with nothing to lose; else `NSTerminateCancel`, and
/// the editor asks as `SPC q q` does.
extern "C-unwind" fn should_terminate(_: *mut AnyObject, _: Sel, _: *mut AnyObject) -> usize {
    const CANCEL: usize = 0;
    const NOW: usize = 1;
    if !UNSAVED.load(Ordering::Relaxed) {
        return NOW;
    }
    let asked = PROXY.get().is_some_and(|p| p.send_event(FenixUserEvent::QuitAsked).is_ok());
    if asked { CANCEL } else { NOW }
}

/// Files dropped on the Dock icon, or opened with Fenix from Finder.
extern "C-unwind" fn open_urls(_: *mut AnyObject, _: Sel, _: *mut AnyObject, urls: *mut AnyObject) {
    let mut paths = Vec::new();
    // SAFETY: AppKit gives an `NSArray<NSURL *>`; each string read is
    // copied before the next message.
    unsafe {
        let count: usize = msg_send![urls, count];
        for i in 0..count {
            let url: *mut AnyObject = msg_send![urls, objectAtIndex: i];
            let file: bool = msg_send![url, isFileURL];
            if !file {
                continue;
            }
            let text: *const c_char = msg_send![url, fileSystemRepresentation];
            if !text.is_null() {
                paths.push(PathBuf::from(OsString::from_vec(CStr::from_ptr(text).to_bytes().to_vec())));
            }
        }
    }
    if let (false, Some(proxy)) = (paths.is_empty(), PROXY.get()) {
        let _ = proxy.send_event(FenixUserEvent::OpenAsked(paths));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_is_what_s_between_the_marks_whatever_a_profile_prints() {
        let text = format!("Welcome!\n{MARK}PATH=/a:/b\0MULTI=one\ntwo=2\0=odd\0{MARK}bye");
        let env = parse_environment(between_marks(text.as_bytes()).unwrap());
        assert_eq!(env[&OsString::from("PATH")], "/a:/b");
        assert_eq!(env[&OsString::from("MULTI")], "one\ntwo=2");
        assert_eq!(env.len(), 2);
        assert_eq!(between_marks(format!("{MARK}PATH=/a").as_bytes()), None, "cut short");
    }

    /// The real shell, whichever it is.
    #[test]
    fn a_login_shell_says_its_path() {
        let env = parse_environment(&login_environment(Duration::from_secs(20)).expect("the shell answers"));
        assert!(env.contains_key(&OsString::from("PATH")));
    }
}
