//! The host half of the stream page (`stream_page`): a recording read
//! off the UI thread, and live sources -- TCP, UDP, a file still being
//! written, a NATS subject -- received on a thread each, both through
//! `fenix_ccsds::stream::Splitter`, each packet summarized through the
//! MIB into a row. Fenix only receives: nothing here sends a byte of
//! telemetry or a telecommand anywhere (NATS gets its protocol's own
//! CONNECT, SUB and PONG, nothing else).

use std::io::{BufRead, Read, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use fenix_ccsds::field::Check;
use fenix_ccsds::stream::{self, Framing, Item, Splitter};
use fenix_mib::{DefRef, Kind, MibSet};

use super::ccsds_host::Mission;
use super::pages::{PageEvent, PageModel, Sender};
use super::*;
use crate::stream_page::{self, Row, Sample, StreamPage};

/// Most rows a page keeps; a live page drops its oldest past this.
const MAX_ROWS: usize = 500_000;

/// A packet's row: what it is, per the MIB, and whether it checks out.
pub(crate) fn row_of(bytes: Vec<u8>, offset: usize, vc: Option<u8>, set: Option<&MibSet>, m: &Mission) -> Row {
    let Some((pkt, sec, root)) = fenix_ccsds::pus::decode(&bytes, &m.profile) else {
        return Row { offset, bad: Some("not a packet".into()), len: bytes.len(), bytes, ..Default::default() };
    };
    let h = pkt.header;
    let bad = root.walk().into_iter().find_map(|(_, f)| match &f.check {
        Some(Check::Bad(why)) => Some(if f.name == "packet error control" { format!("CRC {}, {why}", f.raw) } else { format!("{}: {why}", f.name) }),
        _ => None,
    });
    let time = sec.as_ref().and_then(|s| s.time).map(|t| fenix_ccsds::time::format_utc(fenix_ccsds::time::tai_to_utc(t, &m.profile.leap)));
    let pus = sec.as_ref().map(|s| (s.service, s.subtype));
    let mut spid = None;
    let mut label = match pus {
        Some((s, st)) => fenix_ccsds::pus::subtype_name(s, st).map(str::to_string).unwrap_or_else(|| format!("{s},{st}")),
        None => format!("APID 0x{:03X}", h.apid),
    };
    if let Some(set) = set {
        if h.is_tc {
            if let Some(tc) = fenix_mib::packets::identify_tc(set, &pkt, sec.as_ref(), &m.profile) {
                label = set.get(tc).name.clone();
            }
        } else if let Some(d) = fenix_mib::packets::identify_tm(set, &pkt, sec.as_ref()) {
            let e = set.get(d);
            spid = Some(e.name.clone());
            label = if e.alias.is_empty() { e.description.clone() } else { e.alias.clone() };
        }
    }
    // A verification report starts with the TC's packet ID and sequence control.
    let verifies = match pus {
        Some((1, _)) if !h.is_tc => {
            let head = 6 + fenix_ccsds::pus::tm_header_len(&m.profile);
            bytes.get(head..head + 4).map(|b| {
                let apid = u16::from_be_bytes([b[0], b[1]]) & 0x7FF;
                let seq = u16::from_be_bytes([b[2], b[3]]) & 0x3FFF;
                format!("TC APID 0x{apid:03X} seq {seq}")
            })
        }
        _ => None,
    };
    Row {
        offset,
        time,
        apid: h.apid,
        is_tc: h.is_tc,
        pus,
        seq: h.seq_count,
        len: bytes.len(),
        spid,
        label,
        bad,
        vc,
        idle: h.apid == fenix_ccsds::packet::IDLE_APID,
        verifies,
        bytes,
    }
}

/// Rows, lost frames and skipped bytes out of splitter items.
fn convert(items: Vec<Item>, set: Option<&MibSet>, m: &Mission) -> (Vec<Row>, Vec<(usize, u8, u64)>, Vec<(usize, usize, String)>) {
    let mut rows = Vec::new();
    let mut lost = Vec::new();
    let mut junk = Vec::new();
    for item in items {
        match item {
            Item::Packet { offset, bytes, vc } => rows.push(row_of(bytes, offset, vc, set, m)),
            Item::Frame { offset, info, lost: n, rs, .. } => {
                if n > 0 {
                    lost.push((offset, info.vcid, n));
                }
                if rs == Some(None) {
                    junk.push((offset, 0, "a Reed-Solomon code word was beyond repair".to_string()));
                }
            }
            Item::Junk { offset, len, why } => junk.push((offset, len, why)),
        }
    }
    (rows, lost, junk)
}

/// A source's framing: `guess`, `packets`, `frames` (the project's
/// profile), `records N`.
fn framing_of(text: &str, m: &Mission) -> Option<Framing> {
    let t = text.trim().to_ascii_lowercase();
    match t.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["packets"] => Some(Framing::Packets),
        ["frames"] | ["cadu"] | ["cadus"] => Some(Framing::Frames(m.frames.clone())),
        ["records", n] => n.parse().ok().map(|header| Framing::Records { header }),
        _ => None,
    }
}

/// A guess, reconciled with the project's frame profile when it agrees.
fn guessed(head: &[u8], m: &Mission) -> (Framing, String) {
    let (f, why) = stream::guess(head);
    match f {
        Framing::Frames(g) if g.length == m.frames.length || g.rs_depth == m.frames.rs_depth && g.rs_depth > 0 => {
            (Framing::Frames(FrameProfileExt::merged(&m.frames, &g)), format!("{why}, the project's frames"))
        }
        other => (other, why),
    }
}

struct FrameProfileExt;
impl FrameProfileExt {
    fn merged(project: &fenix_ccsds::frames::FrameProfile, guess: &fenix_ccsds::frames::FrameProfile) -> fenix_ccsds::frames::FrameProfile {
        fenix_ccsds::frames::FrameProfile { asm: guess.asm, randomized: guess.randomized || project.randomized, ..project.clone() }
    }
}

fn send_rows(send: &Sender, buffer: BufferId, items: Vec<Item>, set: Option<&MibSet>, m: &Mission, done: bool) {
    let (rows, lost, junk) = convert(items, set, m);
    if !rows.is_empty() || !lost.is_empty() || !junk.is_empty() || done {
        send(PageEvent::StreamRows { buffer, rows, lost, junk, done });
    }
}

/// Reads a recording in chunks; rows go back as they're found.
fn read_recording(path: PathBuf, framing: Option<Framing>, set: Option<Arc<MibSet>>, m: Mission, buffer: BufferId, send: Sender, stop: Arc<AtomicBool>) {
    let mut file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            send(PageEvent::StreamStatus { buffer, text: format!("couldn't open: {e}"), ok: false });
            send(PageEvent::StreamRows { buffer, rows: Vec::new(), lost: Vec::new(), junk: Vec::new(), done: true });
            return;
        }
    };
    let mut head = vec![0u8; 256 * 1024];
    let n = file.read(&mut head).unwrap_or(0);
    head.truncate(n);
    let (framing, why) = match framing {
        Some(f) => (f, "as asked".to_string()),
        None => guessed(&head, &m),
    };
    send(PageEvent::StreamInfo { buffer, framing: format!("{} · {why}", framing.label()) });
    let mut splitter = Splitter::new(framing);
    let set = set.as_deref();
    let mut items = splitter.feed(&head);
    let mut total = 0;
    let mut chunk = vec![0u8; 1024 * 1024];
    loop {
        total += items.iter().filter(|i| matches!(i, Item::Packet { .. })).count();
        send_rows(&send, buffer, std::mem::take(&mut items), set, &m, false);
        if stop.load(Ordering::Relaxed) || total >= MAX_ROWS {
            if total >= MAX_ROWS {
                send(PageEvent::StreamStatus { buffer, text: format!("stopped at {MAX_ROWS} packets"), ok: false });
            }
            break;
        }
        match file.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => items = splitter.feed(&chunk[..n]),
        }
    }
    send_rows(&send, buffer, splitter.finish(), set, &m, true);
}

/// Where a stream page's packets come from.
#[derive(Debug, Clone)]
pub(crate) enum StreamSource {
    File(PathBuf, Option<Framing>),
    Live(Source),
}

/// A live source's address and how to read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Source {
    pub(crate) name: String,
    pub(crate) address: String,
    pub(crate) subject: String,
    pub(crate) framing: String,
}

/// Receives from `src` until the page closes, reconnecting as needed.
fn run_live(src: Source, set: Option<Arc<MibSet>>, m: Mission, buffer: BufferId, send: Sender, stop: Arc<AtomicBool>) {
    let framing = framing_of(&src.framing, &m).unwrap_or(Framing::Packets);
    send(PageEvent::StreamInfo { buffer, framing: format!("{} · {}", framing.label(), src.address) });
    let mut splitter = Splitter::new(framing);
    let status = |text: String, ok: bool| send(PageEvent::StreamStatus { buffer, text, ok });
    let feed = |bytes: &[u8], splitter: &mut Splitter| {
        let items = splitter.feed(bytes);
        send_rows(&send, buffer, items, set.as_deref(), &m, false);
    };
    let (scheme, rest) = src.address.split_once("://").unwrap_or(("tcp", src.address.as_str()));
    while !stop.load(Ordering::Relaxed) {
        let result: Result<(), String> = match scheme {
            "tcp" => (|| {
                let s = std::net::TcpStream::connect(rest).map_err(|e| e.to_string())?;
                s.set_read_timeout(Some(Duration::from_millis(300))).ok();
                status(format!("{} connected", src.name), true);
                let mut s = s;
                let mut buf = vec![0u8; 64 * 1024];
                while !stop.load(Ordering::Relaxed) {
                    match s.read(&mut buf) {
                        Ok(0) => return Err("closed by the other end".into()),
                        Ok(n) => feed(&buf[..n], &mut splitter),
                        Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                        Err(e) => return Err(e.to_string()),
                    }
                }
                Ok(())
            })(),
            "udp" => (|| {
                let bind = if rest.starts_with(':') { format!("0.0.0.0{rest}") } else { rest.to_string() };
                let s = std::net::UdpSocket::bind(&bind).map_err(|e| e.to_string())?;
                s.set_read_timeout(Some(Duration::from_millis(300))).ok();
                status(format!("{} listening on {bind}", src.name), true);
                let mut buf = vec![0u8; 65_536];
                while !stop.load(Ordering::Relaxed) {
                    match s.recv(&mut buf) {
                        Ok(n) => feed(&buf[..n], &mut splitter),
                        Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                        Err(e) => return Err(e.to_string()),
                    }
                }
                Ok(())
            })(),
            "file" => (|| {
                let path = PathBuf::from(rest);
                let mut f = std::fs::File::open(&path).map_err(|e| e.to_string())?;
                status(format!("{} following {}", src.name, path.display()), true);
                let mut buf = vec![0u8; 1024 * 1024];
                let mut read_so_far: u64 = 0;
                while !stop.load(Ordering::Relaxed) {
                    match f.read(&mut buf) {
                        Ok(0) => {
                            // A file cut short (rotated, rewritten) is read again from its start.
                            if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) < read_so_far {
                                return Err("the file got shorter -- reading it again".into());
                            }
                            std::thread::sleep(Duration::from_millis(250));
                        }
                        Ok(n) => {
                            read_so_far += n as u64;
                            feed(&buf[..n], &mut splitter);
                        }
                        Err(e) => return Err(e.to_string()),
                    }
                }
                Ok(())
            })(),
            "nats" => nats_receive(rest, &src.subject, &stop, &mut |b| feed(b, &mut splitter), &|t| status(format!("{} {t}", src.name), true)),
            other => Err(format!("{other}:// -- tcp, udp, file or nats")),
        };
        match result {
            Ok(()) => break,
            Err(e) => {
                status(format!("{}: {e} -- trying again", src.name), false);
                for _ in 0..10 {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }
}

/// Subscribes to `subject` on the NATS server at `addr` and hands each
/// message's payload to `feed`. The client speaks only what receiving
/// needs: CONNECT, SUB, and PONG to the server's PING.
pub(crate) fn nats_receive(addr: &str, subject: &str, stop: &AtomicBool, feed: &mut dyn FnMut(&[u8]), status: &dyn Fn(String)) -> Result<(), String> {
    if subject.trim().is_empty() {
        return Err("a NATS source needs a subject".into());
    }
    let stream = std::net::TcpStream::connect(addr).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(Duration::from_millis(300))).ok();
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    let mut reader = std::io::BufReader::new(stream);
    writer
        .write_all(format!("CONNECT {{\"verbose\":false,\"pedantic\":false,\"name\":\"fenix\",\"lang\":\"rust\",\"version\":\"1\"}}\r\nSUB {} 1\r\nPING\r\n", subject.trim()).as_bytes())
        .map_err(|e| e.to_string())?;
    status(format!("subscribed to {subject}"));
    let mut line = String::new();
    while !stop.load(Ordering::Relaxed) {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return Err("the server closed the connection".into()),
            Ok(_) => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(e) => return Err(e.to_string()),
        }
        let l = line.trim_end();
        let mut words = l.split_whitespace();
        match words.next().unwrap_or("") {
            "PING" => writer.write_all(b"PONG\r\n").map_err(|e| e.to_string())?,
            "-ERR" => return Err(l.trim_start_matches("-ERR").trim().trim_matches('\'').to_string()),
            "MSG" | "HMSG" => {
                let parts: Vec<&str> = l.split_whitespace().collect();
                let total: usize = parts.last().and_then(|n| n.parse().ok()).ok_or("a MSG without a size")?;
                let header: usize = if parts[0] == "HMSG" { parts.get(parts.len() - 2).and_then(|n| n.parse().ok()).unwrap_or(0) } else { 0 };
                let mut payload = vec![0u8; total + 2];
                let mut got = 0;
                while got < payload.len() {
                    match reader.read(&mut payload[got..]) {
                        Ok(0) => return Err("the server closed the connection".into()),
                        Ok(n) => got += n,
                        Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                            if stop.load(Ordering::Relaxed) {
                                return Ok(());
                            }
                        }
                        Err(e) => return Err(e.to_string()),
                    }
                }
                feed(&payload[header..total]);
            }
            _ => {}
        }
    }
    Ok(())
}

impl App {
    fn stream_page(&mut self, id: BufferId) -> Option<&mut StreamPage> {
        match self.pages.get_mut(&id).map(|s| {
            s.stale = true;
            &mut s.model
        }) {
            Some(PageModel::Stream(p)) => Some(p),
            _ => None,
        }
    }

    /// `SPC k f`: the file in the explorer, or the focused one, as a
    /// recording.
    pub(crate) fn cmd_ccsds_recording(&mut self) {
        let path = if self.main_view == MainView::Explorer { self.explorer_selected_path() } else { self.open().buffer.path().map(Path::to_path_buf) };
        match path.filter(|p| p.is_file()) {
            Some(p) => self.open_recording(p, None),
            None => self.set_error("select a recording in the explorer (SPC f j), then SPC k f"),
        }
    }

    pub(crate) fn open_recording(&mut self, path: PathBuf, framing: Option<Framing>) {
        self.main_view = MainView::Editor;
        let (key, _) = self.mib_key_here();
        let set = if key.roots.is_empty() { None } else { self.mib_set(&key) };
        let mission = self.mission(key.project.as_deref());
        let title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let page = StreamPage::new(key, title, false);
        let stop = page.stop.clone();
        let id = self.open_page(PageModel::Stream(Box::new(page)));
        self.stream_sources.insert(id, StreamSource::File(path.clone(), framing.clone()));
        self.page_spawn(move |send| read_recording(path, framing, set, mission, id, send, stop));
    }

    /// The live sources the project (or you) lists.
    pub(crate) fn ccsds_sources(&self) -> Vec<Source> {
        let rows = match self.project_settings.as_ref().and_then(|(_, p)| p.get("ccsds.sources")) {
            Some(fenix_config::Value::Records(r)) => r.clone(),
            _ => self.config.ccsds_sources.clone(),
        };
        rows.into_iter()
            .filter(|r| r.len() >= 2)
            .map(|r| {
                let mut address = r[1].clone();
                // A file source relative to the project.
                if let (Some(rest), Some(root)) = (address.strip_prefix("file://"), &self.project_root) {
                    if !Path::new(rest).is_absolute() {
                        address = format!("file://{}", root.join(rest).display());
                    }
                }
                Source { name: r[0].clone(), address, subject: r.get(2).cloned().unwrap_or_default(), framing: r.get(3).cloned().unwrap_or_default() }
            })
            .collect()
    }

    /// `SPC k l`: a live source to follow.
    pub(crate) fn cmd_ccsds_live(&mut self) {
        let sources = self.ccsds_sources();
        if sources.is_empty() {
            self.set_error("no live sources -- add one in the project's CCSDS settings (ccsds.sources)");
            return;
        }
        let candidates = sources.iter().enumerate().map(|(i, s)| fenix_picker::Candidate::new(format!("{}  {}  {}", s.name, s.address, s.subject), i)).collect();
        self.enter_picker(ActivePicker::CcsdsSource(fenix_picker::PickerState::new(candidates)));
    }

    pub(crate) fn open_live(&mut self, index: usize) {
        let Some(src) = self.ccsds_sources().into_iter().nth(index) else { return };
        self.main_view = MainView::Editor;
        let (key, _) = self.mib_key_here();
        let set = if key.roots.is_empty() { None } else { self.mib_set(&key) };
        let mission = self.mission(key.project.as_deref());
        let page = StreamPage::new(key, src.name.clone(), true);
        let stop = page.stop.clone();
        let id = self.open_page(PageModel::Stream(Box::new(page)));
        self.stream_sources.insert(id, StreamSource::Live(src.clone()));
        let proxy = self.event_proxy.clone();
        if proxy.is_none() {
            // Tests: no event loop to receive on.
            return;
        }
        self.page_spawn(move |send| run_live(src, set, mission, id, send, stop));
    }

    pub(super) fn apply_stream_event(&mut self, event: PageEvent) {
        match event {
            PageEvent::StreamRows { buffer, rows, lost, junk, done } => {
                let Some(p) = self.stream_page(buffer) else { return };
                p.rows.extend(rows);
                if p.rows.len() > MAX_ROWS {
                    let drop = p.rows.len() - MAX_ROWS;
                    p.rows.drain(..drop);
                }
                p.lost.extend(lost);
                p.junk.extend(junk);
                p.done |= done;
                p.arrived();
            }
            PageEvent::StreamInfo { buffer, framing } => {
                if let Some(p) = self.stream_page(buffer) {
                    p.framing = framing;
                }
            }
            PageEvent::StreamStatus { buffer, text, ok } => {
                if let Some(p) = self.stream_page(buffer) {
                    p.status = Some((text, ok));
                }
            }
            _ => {}
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    pub(super) fn stream_page_action(&mut self, id: BufferId, action: stream_page::Action) {
        use stream_page::Action as A;
        match action {
            A::None => {}
            A::Close => {
                if let Some(p) = self.stream_page(id) {
                    p.close();
                }
                self.stream_sources.remove(&id);
                self.close_page(id);
            }
            A::Inspect(row) => {
                let Some(p) = self.stream_page(id) else { return };
                let Some(r) = p.rows.get(row) else { return };
                let (bytes, source) = (r.bytes.clone(), format!("{} @0x{:X}", p.title, r.offset));
                self.open_packet_page(bytes, source);
            }
            A::Hex(offset) => {
                if let Some(StreamSource::File(path, _)) = self.stream_sources.get(&id).cloned() {
                    self.open_hex_view(path);
                    let hex = self.focused_buffer_id();
                    if let Some(PageModel::Hex(h)) = self.pages.get_mut(&hex).map(|s| &mut s.model) {
                        h.goto(offset);
                    }
                } else {
                    self.set_message("a live source has no file to show as hex -- w writes one");
                }
            }
            A::PickParameter => {
                let Some(key) = self.stream_page(id).map(|p| p.key.clone()) else { return };
                let Some(set) = self.mib_set_ready(&key) else {
                    self.set_error("no MIB for this project, so no parameters to follow");
                    return;
                };
                let candidates = set
                    .entries(Kind::TmParam)
                    .iter()
                    .enumerate()
                    .map(|(index, e)| fenix_picker::Candidate::new(format!("{:<10}  {}", e.name, e.description), DefRef { kind: Kind::TmParam, index }))
                    .collect();
                self.mib_picker_key = key;
                self.mib_pick_for_stream = Some(id);
                self.enter_picker(ActivePicker::MibDef(fenix_picker::PickerState::new(candidates)));
            }
            A::Write(kind) => self.stream_write(id, kind),
            A::Reframe => {
                let Some(StreamSource::File(path, framing)) = self.stream_sources.get(&id).cloned() else { return };
                let mission = self.mission(None);
                let next = match framing {
                    None => Some(Framing::Packets),
                    Some(Framing::Packets) => Some(Framing::Frames(mission.frames.clone())),
                    Some(Framing::Frames(f)) if f.asm => Some(Framing::Frames(fenix_ccsds::frames::FrameProfile { asm: false, ..f })),
                    Some(_) => None,
                };
                if let Some(p) = self.stream_page(id) {
                    p.close();
                }
                self.stream_sources.remove(&id);
                self.close_page(id);
                self.open_recording(path, next);
            }
        }
    }

    /// The parameter picked on a stream page: its samples.
    pub(super) fn stream_follow(&mut self, id: BufferId, param: DefRef) {
        let Some(key) = self.stream_page(id).map(|p| p.key.clone()) else { return };
        let Some(set) = self.mib_set_ready(&key) else { return };
        let mission = self.mission(key.project.as_deref());
        let name = set.get(param).name.clone();
        let carriers: Vec<String> = set.used_by(param).iter().map(|d| set.get(*d).name.clone()).collect();
        let Some(p) = self.stream_page(id) else { return };
        let mut samples = Vec::new();
        for (i, r) in p.rows.iter().enumerate() {
            if !r.spid.as_ref().is_some_and(|s| carriers.contains(s)) {
                continue;
            }
            let Some((root, _)) = fenix_mib::packets::decode_packet(Some(&set), &r.bytes, &mission.profile, mission.base) else { continue };
            let Some(f) = root.walk().into_iter().map(|(_, f)| f).find(|f| f.link == Some(fenix_ccsds::Link::Parameter(name.clone()))).cloned() else { continue };
            let number = f.value.split_whitespace().next().and_then(|v| v.parse::<f64>().ok());
            let off = match &f.check {
                Some(Check::Warn(m)) => Some((m.clone(), false)),
                Some(Check::Bad(m)) => Some((m.clone(), true)),
                _ => None,
            };
            samples.push(Sample { row: i, time: r.time.clone(), raw: f.raw.clone(), value: f.value.clone(), number, off });
        }
        let n = samples.len();
        p.param = Some((name.clone(), samples));
        p.note = Some((format!("{name}: {n} samples in {} packets carrying it", carriers.join(", ")), n == 0));
        if let Some(p) = self.stream_page(id) {
            p.key(crate::page::Key::Char('3'));
        }
    }

    /// Writes what the page shows beside the recording (or into the
    /// project's `recordings/` for a live source).
    fn stream_write(&mut self, id: BufferId, kind: stream_page::Write) {
        let base = match self.stream_sources.get(&id) {
            Some(StreamSource::File(path, _)) => path.with_extension(""),
            Some(StreamSource::Live(src)) => {
                let dir = self.project_root.clone().unwrap_or_else(|| PathBuf::from(".")).join("recordings");
                let _ = std::fs::create_dir_all(&dir);
                dir.join(format!("{}-{}", src.name, chrono::Local::now().format("%Y%m%d-%H%M%S")))
            }
            None => return,
        };
        let Some(p) = self.stream_page(id) else { return };
        let (path, data): (PathBuf, Vec<u8>) = match kind {
            stream_page::Write::Binary => (PathBuf::from(format!("{}-filtered.bin", base.display())), p.shown().iter().flat_map(|&i| p.rows[i].bytes.clone()).collect()),
            stream_page::Write::Hex => (
                PathBuf::from(format!("{}-filtered.hex.txt", base.display())),
                p.shown().iter().map(|&i| format!("{:08X}  {}\n", p.rows[i].offset, fenix_ccsds::field::hex(&p.rows[i].bytes))).collect::<String>().into_bytes(),
            ),
            stream_page::Write::Csv => {
                let Some((name, samples)) = &p.param else { return };
                let mut csv = String::from("time,raw,value,limits\n");
                for s in samples {
                    csv.push_str(&format!("{},{},\"{}\",\"{}\"\n", s.time.clone().unwrap_or_default(), s.raw, s.value, s.off.as_ref().map(|o| o.0.clone()).unwrap_or_default()));
                }
                (PathBuf::from(format!("{}-{name}.csv", base.display())), csv.into_bytes())
            }
        };
        match std::fs::write(&path, data) {
            Ok(()) => self.set_message(format!("wrote {}", path.display())),
            Err(e) => self.set_error(format!("couldn't write {}: {e}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nats_subscription_receives_message_payloads() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(b"INFO {\"server_id\":\"test\"}\r\n").unwrap();
            let mut r = std::io::BufReader::new(s.try_clone().unwrap());
            let mut got = String::new();
            for _ in 0..3 {
                let mut l = String::new();
                r.read_line(&mut l).unwrap();
                got.push_str(&l);
            }
            s.write_all(b"PING\r\nMSG tm.ops 1 4\r\n\x0b\xf2\xc1\x23\r\nMSG tm.ops 1 2\r\nab\r\n").unwrap();
            let mut pong = String::new();
            r.read_line(&mut pong).unwrap();
            (got, pong)
        });
        let stop = AtomicBool::new(false);
        let mut payloads: Vec<Vec<u8>> = Vec::new();
        let mut feed = |b: &[u8]| {
            payloads.push(b.to_vec());
            if payloads.len() == 2 {
                stop.store(true, Ordering::Relaxed);
            }
        };
        let r = nats_receive(&addr, "tm.ops", &stop, &mut feed, &|_| {});
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(payloads, vec![vec![0x0b, 0xf2, 0xc1, 0x23], b"ab".to_vec()]);
        let (sent, pong) = server.join().unwrap();
        assert!(sent.starts_with("CONNECT {") && sent.contains("SUB tm.ops 1\r\n") && sent.contains("PING\r\n"), "{sent}");
        assert_eq!(pong, "PONG\r\n");
    }

    /// The design document's example packet, with sequence count `seq`.
    fn tm(seq: u16) -> Vec<u8> {
        let mut b = vec![
            0x0B, 0xF2, 0xC1, 0x23, 0x00, 0x16, 0x20, 0x03, 0x19, 0x00, 0x42, 0x00, 0x00, 0x81, 0x4B, 0x87, 0x8A, 0x80, 0x00, 0x00, 0x01, 0x01, 0x0B, 0xB8, 0x0A,
            0x28, 0x02,
        ];
        b[2] = 0xC0 | (seq >> 8) as u8;
        b[3] = seq as u8;
        let crc = fenix_ccsds::crc::ccitt16(&b);
        b.extend(crc.to_be_bytes());
        b
    }

    #[test]
    fn a_recording_opens_with_its_gaps_and_a_parameter_can_be_followed() {
        let dir = std::env::temp_dir().join(format!("fenix-stream-host-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("ops")).unwrap();
        std::fs::create_dir_all(dir.join(".fenix")).unwrap();
        let dir = fenix_lsp::normalize(std::fs::canonicalize(&dir).unwrap());
        let w = |t: &str, c: &str| std::fs::write(dir.join("ops").join(format!("{t}.dat")), c).unwrap();
        w("pic", "3\t25\t19\t16\t-1\t0\t\n");
        w("pid", "3\t25\t1010\t1\t0\t30211\tTCS fast housekeeping\t\t\t13\tY\t\t\t1\t\t\n");
        w("tpcf", "30211\tTCS_HK_FAST\t29\n");
        w("pcf", "NTH00123\tHeater 3 temperature\t\tdegC\t3\t12\t\t\t\tN\tR\tCAF00310\n");
        w("plf", "NTH00123\t30211\t3\t0\n");
        w("caf", "CAF00310\tThermistor curve\tR\tU\tD\tdegC\t2\t\n");
        w("cap", "CAF00310\t0\t-40\nCAF00310\t3000\t45\n");
        w("ocp", "NTH00123\t1\tS\t-10\t40\t\t\n");
        std::fs::write(dir.join(".fenix").join("settings.toml"), "[mib.roots]\nOPS = \"ops\"\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "x\n").unwrap();
        let rec = dir.join("run.bin");
        let mut bytes = Vec::new();
        for s in [0x123, 0x124, 0x128] {
            bytes.extend(tm(s));
        }
        bytes[29 + 28] ^= 0xFF; // the second packet's CRC fails
        std::fs::write(&rec, &bytes).unwrap();

        let mut app = App::with_file(Some(dir.join("notes.txt").display().to_string()));
        app.refresh_project_root();
        app.open_recording(rec.clone(), None);
        let id = app.focused_buffer_id();
        let p = app.stream_page(id).unwrap();
        assert_eq!(p.rows.len(), 3);
        assert_eq!(p.rows[0].spid.as_deref(), Some("30211"));
        assert!(p.rows[1].bad.as_deref().is_some_and(|b| b.starts_with("CRC")), "{:?}", p.rows[1].bad);
        assert!(p.done);
        let text = stream_page::layout(p, 160).text;
        assert!(text.contains("3 missing on APID 0x3F2"), "{text}");

        let key = app.stream_page(id).unwrap().key.clone();
        let set = app.mib_set_ready(&key).unwrap();
        let param = set.find(Kind::TmParam, 0, "NTH00123").unwrap();
        app.stream_follow(id, param);
        let (name, samples) = app.stream_page(id).unwrap().param.clone().unwrap();
        assert_eq!(name, "NTH00123");
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[0].value, "45.0 degC");
        assert!(samples[0].off.is_some(), "over the soft 40");
        app.stream_write(id, stream_page::Write::Csv);
        let csv = std::fs::read_to_string(dir.join("run-NTH00123.csv")).unwrap();
        assert!(csv.starts_with("time,raw,value,limits\n2026-09-27 14:32:05.500,3000,\"45.0 degC\""), "{csv}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn framing_names_read() {
        let m = App::with_file(None).mission(None);
        assert_eq!(framing_of("packets", &m), Some(Framing::Packets));
        assert_eq!(framing_of("records 12", &m), Some(Framing::Records { header: 12 }));
        assert!(matches!(framing_of("frames", &m), Some(Framing::Frames(_))));
        assert_eq!(framing_of("guess", &m), None);
    }
}
