//! The explorers' folders watched for names added, removed or renamed --
//! by a terminal, an agent, a save as, `git checkout` -- so the sidebar
//! and the explorer pages catch up at once, not on the next disk poll.
//!
//! The folders on screen are watched one by one (not recursively: a
//! listing shows the folders open in it, and a whole tree's watches would
//! be thousands). Only a name's coming, going or changing counts: reading
//! a folder raises events of its own on some systems, and a listing that
//! read itself again for those would never stop. A burst of changes (a
//! checkout, a build) is read once, `SETTLE` after the first.
//!
//! The disk poll's look at the folders' times (`ExplorerStamps`) stays:
//! a share or a remote machine sends no notifications, and a watch that
//! couldn't be set up (out of inotify watches) still has it to fall back
//! on.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};

use super::*;

/// How long after a change the listings it touches are read again.
pub(super) const SETTLE: Duration = Duration::from_millis(150);

/// The watcher and what it's watching.
pub(crate) struct ExplorerWatch {
    watcher: notify::RecommendedWatcher,
    watched: HashSet<PathBuf>,
}

/// The folders whose names an event changed: each path's folder, for a
/// name made, removed or renamed. Anything else -- a folder read, a file
/// written to, its times -- changes no listing.
pub(super) fn folders_changed(event: &notify::Event) -> Vec<PathBuf> {
    let names = matches!(event.kind, EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(notify::event::ModifyKind::Name(_)));
    if !names {
        return Vec::new();
    }
    let mut out: Vec<PathBuf> = event.paths.iter().filter_map(|p| p.parent().map(Path::to_path_buf)).collect();
    out.sort();
    out.dedup();
    out
}

impl App {
    /// The watches made to match the folders on screen: new ones watched,
    /// those no longer shown let go. Without an event loop (every test)
    /// there's nowhere to send what's seen, and nothing is watched.
    pub(super) fn explorer_watch_sync(&mut self) {
        let Some(proxy) = self.event_proxy.clone() else { return };
        let wanted: HashSet<PathBuf> = self.explorer_folders_to_watch().into_iter().flat_map(|(_, dirs)| dirs).collect();
        if wanted.is_empty() && self.explorer_watch.is_none() {
            return;
        }
        if self.explorer_watch.is_none() {
            let made = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event {
                    let folders = folders_changed(&event);
                    if !folders.is_empty() {
                        let _ = proxy.send_event(FenixUserEvent::ExplorerFoldersChanged(folders));
                    }
                }
            });
            match made {
                Ok(watcher) => self.explorer_watch = Some(ExplorerWatch { watcher, watched: HashSet::new() }),
                // The poll still catches changes, a little later.
                Err(err) => {
                    eprintln!("fenix: couldn't watch the explorers' folders ({err}); the disk poll catches changes instead");
                    return;
                }
            }
        }
        let Some(watch) = self.explorer_watch.as_mut() else { return };
        let gone: Vec<PathBuf> = watch.watched.difference(&wanted).cloned().collect();
        for dir in gone {
            let _ = watch.watcher.unwatch(&dir);
            watch.watched.remove(&dir);
        }
        for dir in wanted {
            if !watch.watched.contains(&dir) && watch.watcher.watch(&dir, RecursiveMode::NonRecursive).is_ok() {
                watch.watched.insert(dir);
            }
        }
    }

    /// `FenixUserEvent::ExplorerFoldersChanged`: the listings showing one
    /// of `folders` are read again once the burst has settled.
    pub(super) fn explorer_folders_changed(&mut self, folders: Vec<PathBuf>, now: Instant) {
        let keys: HashSet<PathBuf> = folders.iter().map(|f| place_key(f)).collect();
        for (target, dirs) in self.explorer_folders_to_watch() {
            if dirs.iter().any(|d| keys.contains(&place_key(d))) {
                self.explorer_dirty.insert(target);
            }
        }
        if !self.explorer_dirty.is_empty() && self.explorer_dirty_due.is_none() {
            self.explorer_dirty_due = Some(now + SETTLE);
        }
    }

    /// Once a burst has settled: the listings it touched read again -- not
    /// one being read already, or with a name being typed in it, which
    /// wait for the next change or the poll. When to look again, while
    /// some wait.
    pub(super) fn explorer_relist_dirty(&mut self, now: Instant) -> Option<Instant> {
        let due = self.explorer_dirty_due?;
        if now < due {
            return Some(due);
        }
        self.explorer_dirty_due = None;
        for target in std::mem::take(&mut self.explorer_dirty) {
            let Some(cwd) = self.explorer_state(target).map(|s| s.cwd.clone()) else { continue };
            let reading = self.explorer_requests.get(&target).is_some_and(|r| !r.done);
            if !reading && !self.explorer_editing(target) {
                self.explorer_navigate(target, &cwd);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, ModifyKind, RemoveKind, RenameMode};

    fn event(kind: EventKind, paths: &[&str]) -> notify::Event {
        let mut e = notify::Event::new(kind);
        for p in paths {
            e = e.add_path(PathBuf::from(p));
        }
        e
    }

    #[test]
    fn only_names_coming_going_or_changing_count() {
        assert_eq!(folders_changed(&event(EventKind::Create(CreateKind::File), &["/p/src/new.rs"])), vec![PathBuf::from("/p/src")]);
        assert_eq!(folders_changed(&event(EventKind::Remove(RemoveKind::Any), &["/p/old.md"])), vec![PathBuf::from("/p")]);
        assert_eq!(
            folders_changed(&event(EventKind::Modify(ModifyKind::Name(RenameMode::Both)), &["/p/a.rs", "/p/src/a.rs"])),
            vec![PathBuf::from("/p"), PathBuf::from("/p/src")],
            "a move: both folders"
        );
        assert!(folders_changed(&event(EventKind::Access(AccessKind::Any), &["/p/src"])).is_empty(), "a folder read");
        assert!(folders_changed(&event(EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Any)), &["/p/a.rs"])).is_empty(), "a file written to");
    }
}
