//! The disk poll's file checks, off the UI thread.
//!
//! Every couple of seconds Fenix looks at what's open -- the files, the
//! PDFs, the MIBs' tables, the project's own settings -- to notice when
//! something else changed them. Each look is a `stat`, which is quick on a
//! local disk and can take twenty seconds or more on a share whose server
//! isn't answering: right after a wake, before the VPN or the drive has
//! reconnected. Done on the UI thread, every tick then froze the window
//! for that long, and the next tick came before it had caught up.
//!
//! So the tick only gathers what to look at; a worker thread does the
//! looking and hands back what it saw, and the tick after that acts on
//! it. One probe at a time: a tick that comes while one is still out does
//! nothing, so a stuck share costs one waiting thread, not one per tick.
//! And a volume that has just taken too long is left alone for a while
//! (`Volumes`) -- its files are reported as "not looked at", which is not
//! the same as "gone".

use super::*;
use crate::mib_page::MibKey;

/// A stat slower than this marks its volume as not answering.
const SLOW_STAT: Duration = Duration::from_secs(2);
/// How long a volume that didn't answer is left alone, at first; each
/// further slow answer doubles it, up to `MAX_BACKOFF`.
const FIRST_BACKOFF: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(600);

/// The volumes that have been slow to answer: until when each is left
/// alone, and how long the next wait is. Shared by every thread that
/// looks at files for a tick (the probe, Home's recent files).
fn volumes() -> &'static Mutex<HashMap<PathBuf, (Instant, Duration)>> {
    static VOLUMES: std::sync::OnceLock<Mutex<HashMap<PathBuf, (Instant, Duration)>>> = std::sync::OnceLock::new();
    VOLUMES.get_or_init(Default::default)
}

/// What `path` is on: `\\server\share` or `C:\` -- the unit that stops
/// answering together.
fn volume_of(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => out.push(component),
            _ => break,
        }
    }
    out
}

/// `std::fs::metadata`, unless `path`'s volume was too slow a moment ago:
/// `None` then, for "not looked at". A slow answer puts the volume on
/// the list for next time.
pub(super) fn stat(path: &Path) -> Option<std::io::Result<std::fs::Metadata>> {
    let volume = volume_of(path);
    if let Some((until, _)) = volumes().lock().unwrap_or_else(|e| e.into_inner()).get(&volume) {
        if Instant::now() < *until {
            return None;
        }
    }
    let started = Instant::now();
    let result = std::fs::metadata(path);
    let took = started.elapsed();
    let mut volumes = volumes().lock().unwrap_or_else(|e| e.into_inner());
    if took >= SLOW_STAT {
        let wait = volumes.get(&volume).map(|(_, wait)| (*wait * 2).min(MAX_BACKOFF)).unwrap_or(FIRST_BACKOFF);
        volumes.insert(volume, (Instant::now() + wait, wait));
    } else {
        volumes.remove(&volume);
    }
    Some(result)
}

/// What one tick asks to be looked at.
#[derive(Default)]
pub(super) struct ProbeRequest {
    buffers: Vec<(BufferId, PathBuf)>,
    pdfs: Vec<(BufferId, PathBuf)>,
    mibs: Vec<MibKey>,
    project: Option<PathBuf>,
}

/// What was seen. A file missing from `buffers` or `pdfs` wasn't looked
/// at (its volume isn't answering); `Some(None)` is a file that's gone.
#[derive(Default)]
pub(crate) struct ProbeResult {
    buffers: HashMap<BufferId, Option<DiskFingerprint>>,
    pdfs: HashMap<BufferId, Option<std::time::SystemTime>>,
    mibs: Vec<(MibKey, fenix_mib::Stamp)>,
    project: Option<(PathBuf, fenix_config::ProjectSettings)>,
}

// By hand: the project's settings have no `Debug` of their own.
impl std::fmt::Debug for ProbeResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProbeResult").field("buffers", &self.buffers.len()).field("pdfs", &self.pdfs.len()).field("mibs", &self.mibs.len()).finish()
    }
}

fn run(request: ProbeRequest) -> ProbeResult {
    let _profile = crate::profile::Scope::new("disk probe");
    let mut result = ProbeResult::default();
    for (id, path) in request.buffers {
        if let Some(meta) = stat(&path) {
            result.buffers.insert(id, DiskFingerprint::from_meta(meta.ok()));
        }
    }
    for (id, path) in request.pdfs {
        if let Some(meta) = stat(&path) {
            result.pdfs.insert(id, meta.ok().and_then(|m| m.modified().ok()));
        }
    }
    for key in request.mibs {
        // A MIB on a volume that isn't answering keeps what was read.
        if key.roots.iter().all(|root| stat(&root.path).is_some()) {
            let stamp = fenix_mib::Stamp::of(&key.roots);
            result.mibs.push((key, stamp));
        }
    }
    if let Some(root) = request.project {
        if stat(&root).is_some() {
            let settings = fenix_config::ProjectSettings::load(&root);
            result.project = Some((root, settings));
        }
    }
    result
}

impl App {
    /// The tick's half: asks for a look at everything open, unless the
    /// last look hasn't come back yet.
    pub(super) fn start_disk_probe(&mut self) {
        // One probe at a time. One that never came back (its thread
        // stuck on a share that hasn't answered in minutes) stops holding
        // the others up after a while.
        if self.disk_probe_out.is_some_and(|sent| sent.elapsed() < Duration::from_secs(300)) {
            return;
        }
        let watch = self.config.watch_files.unwrap_or(true);
        let mut request = ProbeRequest::default();
        if watch {
            for id in self.buffers.ids_sorted_by_path() {
                let Some(ob) = self.buffers.get(id) else { continue };
                if !ob.kind.tracks_unsaved_changes() {
                    continue;
                }
                if let Some(path) = ob.buffer.path() {
                    request.buffers.push((id, path.to_path_buf()));
                }
            }
            request.pdfs = self.pdf_docs.iter().map(|(&id, d)| (id, d.path.clone())).collect();
        }
        if self.config.mib_watch.unwrap_or(true) {
            request.mibs = self.mib_sets.iter().filter(|(_, slot)| !slot.loading && slot.set.is_some()).map(|(key, _)| key.clone()).collect();
        }
        request.project = self.project_root.clone();
        self.disk_probe_out = Some(Instant::now());
        self.disk_probe_basis = self.disk_state.clone();
        match self.event_proxy.clone() {
            Some(proxy) => {
                let _ = std::thread::Builder::new().name("fenix-disk-probe".into()).spawn(move || {
                    let result = run(request);
                    let _ = proxy.send_event(FenixUserEvent::DiskProbed(Box::new(result)));
                });
            }
            // No event loop (every test): look and act straight away.
            None => {
                let result = run(request);
                self.apply_disk_probe(result);
            }
        }
    }

    /// What a probe saw, acted on: files changed on disk are reloaded
    /// (or flagged), PDFs read again, MIBs re-read, the project's
    /// settings applied.
    pub(super) fn apply_disk_probe(&mut self, result: ProbeResult) {
        self.disk_probe_out = None;
        if let Some((root, settings)) = result.project {
            self.apply_project_settings(root, settings);
        }
        for (key, stamp) in result.mibs {
            let stale = self.mib_sets.get(&key).is_some_and(|slot| !slot.loading && slot.set.as_ref().is_some_and(|set| *set.stamp() != stamp));
            if stale {
                self.mib_load(key);
            }
        }
        if !self.config.watch_files.unwrap_or(true) {
            return;
        }
        self.pdf_reload_seen(&result.pdfs);
        let sweep = self.reload_buffers_seen(&result.buffers);
        let Some(message) = sweep.message() else { return };
        if sweep.is_bad() {
            self.set_error(message);
        } else {
            self.set_message(message);
        }
        self.wake_caret();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_volume_is_the_drive_or_the_share() {
        assert_eq!(volume_of(Path::new(r"C:\work\a.rs")), PathBuf::from(r"C:\"));
        assert_eq!(volume_of(Path::new(r"\\files\ops\mib\ccf.dat")), PathBuf::from(r"\\files\ops\"));
    }

    #[test]
    fn a_file_saved_while_the_probe_was_out_is_not_called_a_conflict() {
        let dir = std::env::temp_dir().join(format!("fenix-probe-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "one\n").unwrap();
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        let id = app.focused_buffer_id();
        let before = DiskFingerprint::of(&file).unwrap();
        // The probe goes out and sees the file as it is...
        app.disk_probe_basis = app.disk_state.clone();
        // ...then Fenix saves a longer version and goes on typing.
        std::fs::write(&file, "one, saved\n").unwrap();
        app.disk_state.insert(id, DiskFingerprint::of(&file).unwrap());
        app.test_insert_str("more ");
        let sweep = app.reload_buffers_seen(&HashMap::from([(id, Some(before))]));
        assert!(sweep.conflicted.is_empty() && !app.externally_changed.contains(&id), "{sweep:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_volume_that_was_slow_is_left_alone_for_a_while() {
        // A drive letter nothing else uses, so no other test sees it.
        let file = PathBuf::from(r"Q:\fenix-probe\a.txt");
        assert!(matches!(stat(&file), Some(Err(_))), "looked at, and not there");
        let volume = volume_of(&file);
        volumes().lock().unwrap().insert(volume.clone(), (Instant::now() + Duration::from_secs(60), FIRST_BACKOFF));
        assert!(stat(&file).is_none(), "not looked at while it's left alone");
        volumes().lock().unwrap().remove(&volume);
        assert!(stat(&file).is_some());
    }
}
