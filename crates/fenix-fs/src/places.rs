//! Where you can go: the drives, the network drives, and the shares on
//! a server.
//!
//! None of this is in `std::fs`, and all of it is what stands between
//! "type the whole path from memory" and "pick it". The Windows answers
//! come from asking Windows -- one CIM query for every volume at once
//! -- rather than from a binding crate, the same way the rest of this
//! workspace shells out to `git` and `docker` instead of linking them.
//!
//! Everything here can be slow, and `shares` can hang outright on a
//! host that is not there. None of it may be called from a thread that
//! is drawing.

use std::io;
use std::path::PathBuf;
use std::process::Command;

/// What kind of thing a volume is. The distinction that matters is
/// `Network`: it is the one that can stop answering, and the one whose
/// listing should not have `git status` run across it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeKind {
    Fixed,
    Removable,
    Network,
    Optical,
    Other,
}

impl VolumeKind {
    /// Windows' own `Win32_LogicalDisk.DriveType` numbering.
    fn from_drive_type(n: u32) -> Self {
        match n {
            2 => VolumeKind::Removable,
            3 => VolumeKind::Fixed,
            4 => VolumeKind::Network,
            5 => VolumeKind::Optical,
            _ => VolumeKind::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VolumeKind::Fixed => "local disk",
            VolumeKind::Removable => "removable",
            VolumeKind::Network => "network",
            VolumeKind::Optical => "disc",
            VolumeKind::Other => "volume",
        }
    }
}

/// One place a listing can be rooted: a drive, or a drive that is really
/// a share somewhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    /// What to actually open -- `C:\`, or `Z:\` for a mapped share.
    pub root: PathBuf,
    /// The volume's own name, when it has one ("Windows", "Backup").
    /// Empty rather than invented when it does not.
    pub name: String,
    pub kind: VolumeKind,
    /// What a mapped drive points at (`\\nas\media`), so a picker can
    /// show where `Z:` really goes rather than just its letter.
    pub remote: Option<String>,
    /// Bytes free and total. `None` when the volume did not answer --
    /// an empty card reader, a share that is offline -- which is worth
    /// distinguishing from "zero bytes free".
    pub free: Option<u64>,
    pub total: Option<u64>,
}

/// Every volume the machine currently has, newest information first-
/// hand from the platform.
///
/// Falls back to probing drive letters when the query fails or is not
/// available, which loses the sizes and the types but still lets you
/// pick a drive -- the same "degrade rather than disappear" posture the
/// project-file listing already takes when `git` and `fd` are both
/// missing.
pub fn volumes() -> Vec<Volume> {
    #[cfg(windows)]
    {
        let queried = query_windows_volumes();
        if !queried.is_empty() {
            return queried;
        }
        probe_drive_letters()
    }
    #[cfg(not(windows))]
    {
        // One root, and nothing useful to say about it without parsing
        // `/proc/mounts` -- which is a different feature for a different
        // platform, and this one is Windows-first by design.
        vec![Volume { root: PathBuf::from("/"), name: String::new(), kind: VolumeKind::Fixed, remote: None, free: None, total: None }]
    }
}

/// The shares a server is offering, by name.
///
/// For the case the user asked for specifically: you know the host but
/// not the share. Runs `net view`, whose output is localized, so the
/// parsing keys off the row of dashes every locale prints rather than
/// off any word in it.
///
/// **Can block for a long time** on a host that does not exist -- the
/// name resolution and connection attempt behind it have their own
/// timeouts, measured in tens of seconds. Call it where that is
/// survivable.
pub fn shares(server: &str) -> io::Result<Vec<String>> {
    let host = server.trim_start_matches(['\\', '/']).trim_end_matches(['\\', '/']);
    if host.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "no server name"));
    }
    let output = command("net").args(["view", &format!("\\\\{host}")]).output()?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr);
        let message = message.lines().find(|l| !l.trim().is_empty()).unwrap_or("the server did not answer");
        return Err(io::Error::other(message.trim().to_string()));
    }
    Ok(parse_shares(&String::from_utf8_lossy(&output.stdout)))
}

/// Pulls share names out of `net view`'s table.
///
/// Locale-proof by construction: every translation prints the same row
/// of dashes under the headings, so the table starts there, and every
/// one ends the table with a blank line before its closing message.
fn parse_shares(stdout: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_table = false;
    for line in stdout.lines() {
        if !in_table {
            // The separator is dashes and nothing else.
            if line.trim_start().starts_with("---") {
                in_table = true;
            }
            continue;
        }
        if line.trim().is_empty() {
            break;
        }
        let Some(name) = line.split_whitespace().next() else { continue };
        // The closing message is a sentence, not a table row -- it has
        // no second column and usually ends in a period.
        if line.split_whitespace().count() > 4 || name.ends_with('.') {
            break;
        }
        names.push(name.to_string());
    }
    names
}

#[cfg(windows)]
fn query_windows_volumes() -> Vec<Volume> {
    // One call for every volume, with the fields a picker wants. `-sep`
    // rather than JSON or CSV so parsing is a `split`, and `-NoProfile`
    // because a user profile can take over a second to load and has
    // nothing to contribute here.
    const SCRIPT: &str = "Get-CimInstance Win32_LogicalDisk | ForEach-Object { \
         $_.DeviceID + '|' + $_.DriveType + '|' + $_.VolumeName + '|' + $_.FreeSpace + '|' + $_.Size + '|' + $_.ProviderName }";
    let Ok(output) = command("powershell").args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT]).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout).lines().filter_map(parse_volume_row).collect()
}

/// `C:|3|Windows|123456|999999|` -> a `Volume`.
fn parse_volume_row(line: &str) -> Option<Volume> {
    let mut fields = line.trim().split('|');
    let device = fields.next()?.trim();
    if device.is_empty() {
        return None;
    }
    let kind = VolumeKind::from_drive_type(fields.next().unwrap_or("").trim().parse().unwrap_or(0));
    let name = fields.next().unwrap_or("").trim().to_string();
    let free = fields.next().and_then(|v| v.trim().parse().ok());
    let total = fields.next().and_then(|v| v.trim().parse().ok());
    let remote = fields.next().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    // `C:` is a drive; `C:\` is its root directory, and the root is
    // what anything opening it actually wants.
    Some(Volume { root: PathBuf::from(format!("{device}\\")), name, kind, remote, free, total })
}

/// What is left when the query is unavailable: every drive letter that
/// answers, with nothing else known about it. Still enough to pick one.
#[cfg(windows)]
fn probe_drive_letters() -> Vec<Volume> {
    (b'A'..=b'Z')
        .map(|letter| PathBuf::from(format!("{}:\\", letter as char)))
        .filter(|root| root.exists())
        .map(|root| Volume { root, name: String::new(), kind: VolumeKind::Other, remote: None, free: None, total: None })
        .collect()
}

/// A `Command` that does not flash a console window on Windows -- the
/// same `CREATE_NO_WINDOW` treatment `fenix-git`'s own process helper
/// applies, for the same reason: these run while the editor is drawing.
fn command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_drive_type_windows_reports_has_a_meaning() {
        assert_eq!(VolumeKind::from_drive_type(2), VolumeKind::Removable);
        assert_eq!(VolumeKind::from_drive_type(3), VolumeKind::Fixed);
        assert_eq!(VolumeKind::from_drive_type(4), VolumeKind::Network);
        assert_eq!(VolumeKind::from_drive_type(5), VolumeKind::Optical);
        // Anything unexpected is still a volume you can open, which is
        // more useful than dropping it from the list.
        assert_eq!(VolumeKind::from_drive_type(0), VolumeKind::Other);
        assert_eq!(VolumeKind::from_drive_type(6), VolumeKind::Other);
    }

    #[test]
    fn a_volume_row_becomes_something_openable() {
        let volume = parse_volume_row("C:|3|Windows|123456789|999999999|").unwrap();
        // The root directory, not the drive designator -- `C:` on its
        // own means "the current directory on C", which is not what
        // anyone picking a drive means.
        assert_eq!(volume.root, PathBuf::from("C:\\"));
        assert_eq!(volume.name, "Windows");
        assert_eq!(volume.kind, VolumeKind::Fixed);
        assert_eq!(volume.free, Some(123_456_789));
        assert_eq!(volume.total, Some(999_999_999));
        assert_eq!(volume.remote, None);
    }

    #[test]
    fn a_mapped_drive_remembers_where_it_really_points() {
        // Which is the thing worth showing: `Z:` tells you nothing.
        let volume = parse_volume_row(r"Z:|4|  |500|1000|\\nas\media").unwrap();
        assert_eq!(volume.kind, VolumeKind::Network);
        assert_eq!(volume.remote.as_deref(), Some(r"\\nas\media"));
    }

    #[test]
    fn a_volume_that_did_not_answer_reports_nothing_rather_than_zero() {
        // An empty card reader is not a full one.
        let volume = parse_volume_row("D:|5||||").unwrap();
        assert_eq!(volume.kind, VolumeKind::Optical);
        assert_eq!(volume.free, None);
        assert_eq!(volume.total, None);
        assert_eq!(volume.name, "");
    }

    #[test]
    fn blank_and_malformed_rows_are_skipped() {
        assert!(parse_volume_row("").is_none());
        assert!(parse_volume_row("   ").is_none());
        assert!(parse_volume_row("|3|x|1|2|").is_none());
    }

    #[test]
    fn share_names_come_out_of_net_views_table() {
        let stdout = "Shared resources at \\\\nas\n\n\
             Share name  Type  Used as  Comment\n\
             -------------------------------------------------------------\n\
             media       Disk\n\
             public      Disk           Everyone\n\
             backup      Disk\n\
             \n\
             The command completed successfully.\n";
        assert_eq!(parse_shares(stdout), vec!["media", "public", "backup"]);
    }

    #[test]
    fn a_localized_table_parses_the_same_way() {
        // The parsing keys off the row of dashes and the blank line,
        // both of which every translation prints -- nothing here depends
        // on an English word.
        let stdout = "Ressources partagees sur \\\\nas\n\n\
             Nom de partage  Type  Utilise comme  Commentaire\n\
             -------------------------------------------------------------\n\
             media           Disque\n\
             public          Disque\n\
             \n\
             La commande s'est terminee correctement.\n";
        assert_eq!(parse_shares(stdout), vec!["media", "public"]);
    }

    #[test]
    fn a_server_offering_nothing_yields_nothing_rather_than_junk() {
        let stdout = "Shared resources at \\\\nas\n\n\
             Share name  Type  Used as  Comment\n\
             -------------------------------------------------------------\n\
             \n\
             The command completed successfully.\n";
        assert!(parse_shares(stdout).is_empty());
    }

    #[test]
    fn output_with_no_table_at_all_yields_nothing() {
        assert!(parse_shares("System error 53 has occurred.\n").is_empty());
    }

    #[test]
    fn asking_for_the_shares_of_nothing_is_refused_before_running_anything() {
        // Rather than shelling out to `net view \\` and waiting for it
        // to work out that it was asked nonsense.
        assert!(shares("").is_err());
        assert!(shares("\\\\").is_err());
    }

    #[test]
    fn the_machines_own_volumes_are_listed() {
        // Against the real machine: whatever this is running on has at
        // least one volume, and every one of them is openable.
        let volumes = volumes();
        assert!(!volumes.is_empty(), "a machine with no drives at all");
        for volume in &volumes {
            assert!(volume.root.is_absolute(), "not somewhere you could open: {:?}", volume.root);
        }
    }
}
