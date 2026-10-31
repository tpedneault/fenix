//! The processes running on this machine, for `${pickProcess}` in a launch
//! file: a debugger to attach to one. Linux reads `/proc`; macOS
//! asks `ps`; Windows asks `tasklist`.

/// A process: its id, its name, and how it was started, when that's known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    pub command: String,
}

/// This machine's processes, by id, but Fenix itself.
pub fn list() -> Vec<Process> {
    let mut all = read();
    all.retain(|p| p.pid != std::process::id());
    all.sort_by_key(|p| p.pid);
    all
}

#[cfg(target_os = "linux")]
fn read() -> Vec<Process> {
    let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            let name = std::fs::read_to_string(e.path().join("comm")).ok()?.trim().to_string();
            let command = std::fs::read(e.path().join("cmdline")).map(|b| String::from_utf8_lossy(&b).split('\0').filter(|a| !a.is_empty()).collect::<Vec<_>>().join(" ")).unwrap_or_default();
            Some(Process { pid, name, command })
        })
        .collect()
}

#[cfg(all(unix, not(target_os = "linux")))]
fn read() -> Vec<Process> {
    let Ok(out) = std::process::Command::new("ps").args(["-A", "-o", "pid=,comm=,args="]).output() else { return Vec::new() };
    parse_ps(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(windows)]
fn read() -> Vec<Process> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Ok(out) = std::process::Command::new("tasklist").args(["/fo", "csv", "/nh"]).creation_flags(CREATE_NO_WINDOW).output() else { return Vec::new() };
    parse_tasklist(&String::from_utf8_lossy(&out.stdout))
}

/// `ps -A -o pid=,comm=,args=`'s lines: the id, the name (the program's
/// path on macOS: its last part), and the rest.
#[cfg_attr(not(all(unix, not(target_os = "linux"))), allow(dead_code))]
fn parse_ps(text: &str) -> Vec<Process> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let comm = parts.next()?;
            let name = comm.rsplit('/').next().unwrap_or(comm).to_string();
            Some(Process { pid, name, command: parts.collect::<Vec<_>>().join(" ") })
        })
        .collect()
}

/// `tasklist /fo csv /nh`'s lines: `"name","pid",...`.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_tasklist(text: &str) -> Vec<Process> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split("\",\"").map(|f| f.trim_matches(['"', '\r'])).collect();
            let name = fields.first()?.to_string();
            let pid = fields.get(1)?.parse().ok()?;
            Some(Process { pid, command: name.clone(), name })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_machines_processes_are_listed_without_fenix() {
        let all = list();
        assert!(!all.is_empty());
        assert!(all.iter().all(|p| p.pid != std::process::id()));
    }

    #[test]
    fn ps_and_tasklist_are_read() {
        let ps = parse_ps("  1 /sbin/launchd /sbin/launchd\n 812 /usr/bin/dotnet dotnet run --project api\nbad\n");
        assert_eq!(ps[1], Process { pid: 812, name: "dotnet".into(), command: "dotnet run --project api".into() });
        assert_eq!(ps.len(), 2);
        let tl = parse_tasklist("\"System Idle Process\",\"0\",\"Services\",\"0\",\"8 K\"\r\n\"node.exe\",\"4242\",\"Console\",\"1\",\"40,000 K\"\r\n");
        assert_eq!(tl[1].pid, 4242);
        assert_eq!(tl[1].name, "node.exe");
    }
}
