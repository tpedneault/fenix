//! A sound an extension plays (`play-sound`): a WAV, by the platform's own
//! player -- Windows' `PlaySound`; `paplay`, `pw-play` or `aplay` on Linux;
//! `afplay` on macOS -- so Fenix carries no audio stack of its own.
//!
//! The WAV comes as its bytes, read as its path was checked, and
//! goes to the player as bytes -- in memory, or on its stdin -- not by a
//! path a link could have taken the place of since. `afplay` reads only a
//! file: one of Fenix's own, made for it and taken away after.

/// Plays a WAV, once, without waiting for it.
#[cfg_attr(test, allow(dead_code))]
pub fn play_bytes(bytes: Vec<u8>) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::thread::Builder::new().name("fenix-sound".into()).spawn(move || play_windows(&bytes)).map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        play_from_file("afplay", &bytes)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        spawn_first(&[("paplay", &[]), ("pw-play", &["-"]), ("aplay", &["-q"])], bytes)
    }
}

#[cfg(windows)]
fn play_windows(bytes: &[u8]) {
    use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_MEMORY, SND_NODEFAULT};
    // SAFETY: with SND_MEMORY the first argument is the WAV in memory;
    // `bytes` lives until it has played (to its end, here, on a thread of
    // its own).
    unsafe {
        PlaySoundW(bytes.as_ptr().cast(), std::ptr::null_mut(), SND_MEMORY | SND_NODEFAULT);
    }
}

/// `player` started on a file of Fenix's own holding `bytes`, which goes
/// once it's played.
#[cfg(target_os = "macos")]
fn play_from_file(player: &str, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("fenix-sound-{}-{n}.wav", std::process::id()));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    drop(file);
    match Command::new(player).arg(&path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
                let _ = std::fs::remove_file(&path);
            });
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            Err(format!("{player}: {e}"))
        }
    }
}

/// The first of `players` there is, started with its arguments and given
/// `bytes` on its stdin, and waited for on a thread of its own.
#[cfg(not(any(windows, target_os = "macos")))]
fn spawn_first(players: &[(&str, &[&str])], bytes: Vec<u8>) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    for (player, args) in players {
        let mut command = Command::new(player);
        command.args(*args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        match command.spawn() {
            Ok(mut child) => {
                let stdin = child.stdin.take();
                std::thread::spawn(move || {
                    if let Some(mut stdin) = stdin {
                        let _ = stdin.write_all(&bytes);
                    }
                    let _ = child.wait();
                });
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("{player}: {e}")),
        }
    }
    let names: Vec<&str> = players.iter().map(|(p, _)| *p).collect();
    Err(format!("nothing here plays sounds ({})", names.join(", ")))
}

#[cfg(all(test, not(windows), not(target_os = "macos")))]
mod tests {
    use super::*;

    /// The first player there is plays it, from its stdin; with none, it
    /// says so.
    #[test]
    fn the_first_player_there_is_plays_it() {
        let wav = b"RIFF....WAVE".to_vec();
        assert!(spawn_first(&[("fenix-no-such-player", &[])], wav.clone()).unwrap_err().contains("nothing here plays sounds"));
        assert_eq!(spawn_first(&[("fenix-no-such-player", &[]), ("cat", &[])], wav), Ok(()));
    }
}
