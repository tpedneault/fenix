//! The CCSDS side of the mission tools: the profile a project's packets
//! and frames are read with (its `[ccsds]` settings over yours), and the
//! commands that turn bytes into fields -- the inspector, the time
//! converter, the hex view, recordings and live sources.

use std::path::Path;

use fenix_ccsds::field::{Check, Field, Link};
use fenix_ccsds::frames::{FrameKind, FrameProfile};
use fenix_ccsds::pus::{Pec, Profile, PusEdition};
use fenix_ccsds::time::{Epoch, LeapTable, TimeFormat};
use fenix_mib::packets::OffsetBase;
use fenix_mib::{DefRef, Kind, MibSet};

use super::pages::PageModel;
use super::*;
use crate::mib_page::MibKey;
use crate::hex_page::{self, HexPage};
use crate::packet_page::{self, DecodeAs, PacketPage};
use crate::time_page::{self, TimePage};

/// Everything a project says about how its bytes are laid out.
#[derive(Debug, Clone)]
pub(crate) struct Mission {
    pub(crate) profile: Profile,
    pub(crate) base: OffsetBase,
    pub(crate) frames: FrameProfile,
    /// Telecommand frames, as CLTUs carry them.
    pub(crate) tc: FrameProfile,
    /// The nominal epoch, when a time correlation moved the profile's.
    pub(crate) nominal_epoch: Option<Epoch>,
    pub(crate) checks_off: Vec<String>,
    /// What didn't parse, to say once.
    pub(crate) problems: Vec<String>,
}

impl App {
    /// The mission profile for `project` (the focused one when `None`
    /// is passed and a project is open): its settings over yours over
    /// the defaults.
    pub(crate) fn mission(&self, project: Option<&Path>) -> Mission {
        let loaded;
        let ps: Option<&fenix_config::ProjectSettings> = match project {
            Some(root) if self.project_settings.as_ref().is_some_and(|(r, _)| r == root) => self.project_settings.as_ref().map(|(_, p)| p),
            Some(root) => {
                loaded = fenix_config::ProjectSettings::load(root);
                Some(&loaded)
            }
            None => self.project_settings.as_ref().map(|(_, p)| p),
        };
        let text = |key: &str, mine: &Option<String>| -> Option<String> {
            match ps.and_then(|p| p.get(key)) {
                Some(fenix_config::Value::Text(t)) => Some(t.clone()),
                _ => mine.clone(),
            }
        };
        let int = |key: &str, mine: Option<i64>| -> Option<i64> {
            match ps.and_then(|p| p.get(key)) {
                Some(fenix_config::Value::Int(n)) => Some(*n),
                _ => mine,
            }
        };
        let flag = |key: &str, mine: Option<bool>, default: bool| -> bool {
            match ps.and_then(|p| p.get(key)) {
                Some(fenix_config::Value::Bool(b)) => *b,
                _ => mine.unwrap_or(default),
            }
        };
        let c = &self.config;
        let mut problems = Vec::new();
        let leap = match c.ccsds_leap_seconds.as_ref().map(std::fs::read_to_string) {
            Some(Ok(t)) => LeapTable::parse(&t).unwrap_or_else(|e| {
                problems.push(format!("ccsds.leap_seconds: {e}"));
                LeapTable::default()
            }),
            Some(Err(e)) => {
                problems.push(format!("ccsds.leap_seconds: {e}"));
                LeapTable::default()
            }
            None => LeapTable::default(),
        };
        let pus = match text("ccsds.pus", &c.ccsds_pus).as_deref() {
            Some("a") => PusEdition::A,
            Some("none") => PusEdition::None,
            _ => PusEdition::C,
        };
        let tm_time = TimeFormat::parse(&text("ccsds.tm_time", &c.ccsds_tm_time).unwrap_or_else(|| "cuc 4.2".into())).unwrap_or_else(|e| {
            problems.push(format!("ccsds.tm_time: {e}"));
            TimeFormat::Cuc { coarse: 4, fine: 2, pfield: false }
        });
        let epoch = match text("ccsds.epoch", &c.ccsds_epoch) {
            Some(e) => Epoch::parse(&e, &leap).unwrap_or_else(|why| {
                problems.push(format!("ccsds.epoch: {why}"));
                Epoch::default()
            }),
            None => Epoch::default(),
        };
        let tm_time_for_correlation = TimeFormat::parse(&text("ccsds.tm_time", &c.ccsds_tm_time).unwrap_or_else(|| "cuc 4.2".into())).unwrap_or(TimeFormat::Cuc { coarse: 4, fine: 2, pfield: false });
        // A correlation says where the on-board clock really counts from.
        let (epoch, nominal_epoch) = match text("ccsds.time_correlation", &c.ccsds_time_correlation).filter(|t| !t.trim().is_empty()) {
            Some(t) => match fenix_ccsds::time::correlate(&t, tm_time_for_correlation, &leap) {
                Ok(correlated) => (correlated, Some(epoch)),
                Err(why) => {
                    problems.push(format!("ccsds.time_correlation: {why}"));
                    (epoch, None)
                }
            },
            None => (epoch, None),
        };
        let pec = Pec::parse(&text("ccsds.crc", &c.ccsds_crc).unwrap_or_default()).unwrap_or(Pec::Ccitt16);
        let pec = if text("ccsds.crc", &c.ccsds_crc).is_none() { Pec::Ccitt16 } else { pec };
        let base = OffsetBase::parse(&text("ccsds.plf_offset", &c.ccsds_plf_offset).unwrap_or_default()).unwrap_or_default();
        let profile = Profile { pus, tm_time, epoch, leap, pec, tc_source_id: int("ccsds.tc_source_id", c.ccsds_tc_source_id).unwrap_or(0).clamp(0, 65535) as u16, ..Default::default() };
        let vc_names = match ps.and_then(|p| p.get("ccsds.vc_names")) {
            Some(fenix_config::Value::Map(m)) => m.clone(),
            _ => c.ccsds_vc_names.clone(),
        };
        let frames = FrameProfile {
            kind: FrameKind::parse(&text("ccsds.frame_type", &c.ccsds_frame_type).unwrap_or_else(|| "tm".into())).unwrap_or_default(),
            length: int("ccsds.frame_length", c.ccsds_frame_length).unwrap_or(1115).max(7) as usize,
            asm: flag("ccsds.frame_asm", c.ccsds_frame_asm, true),
            randomized: flag("ccsds.frame_randomized", c.ccsds_frame_randomized, false),
            rs_depth: int("ccsds.frame_rs_depth", c.ccsds_frame_rs_depth).unwrap_or(0).clamp(0, 8) as usize,
            ocf: flag("ccsds.frame_ocf", c.ccsds_frame_ocf, true),
            fecf: flag("ccsds.frame_fecf", c.ccsds_frame_fecf, false),
            vc_names: vc_names.into_iter().filter_map(|(k, v)| Some((k.trim().parse().ok()?, v))).collect(),
            ..Default::default()
        };
        let tc = FrameProfile {
            kind: FrameKind::Tc,
            asm: false,
            ocf: false,
            fecf: flag("ccsds.tc_fecf", c.ccsds_tc_fecf, true),
            tc_segment_header: flag("ccsds.tc_segment_header", c.ccsds_tc_segment_header, true),
            randomized: flag("ccsds.tc_randomized", c.ccsds_tc_randomized, false),
            bch_correct: text("ccsds.tc_bch", &c.ccsds_tc_bch).as_deref() != Some("detect"),
            vc_names: frames.vc_names.clone(),
            ..Default::default()
        };
        let checks_off = match ps.and_then(|p| p.get("ccsds.checks_off")) {
            Some(fenix_config::Value::List(l)) => l.clone(),
            _ => c.ccsds_checks_off.clone(),
        };
        Mission { profile, base, frames, tc, nominal_epoch, checks_off, problems }
    }
}

impl App {
    /// Bytes to decode: the Visual selection, the hex run at the cursor,
    /// or the clipboard -- and where they came from.
    fn ccsds_bytes_here(&mut self) -> Option<(Vec<u8>, String)> {
        let name = self.open().buffer.path().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "buffer".into());
        let cursor = self.cursor();
        if self.vim.mode() == Mode::Visual {
            let ob = self.open();
            let text = self.vim.visual_selection_range(&ob.buffer, &cursor).map(|(r, _)| ob.buffer.text_range(r.start, r.end));
            let line = ob.buffer.line_col(&cursor).0 + 1;
            self.vim.exit_visual_mode(&cursor);
            if let Some(bytes) = text.as_deref().and_then(fenix_ccsds::hex::parse) {
                return Some((bytes, format!("{name}:{line}")));
            }
        }
        if !self.is_page_buffer(self.focused_buffer_id()) {
            let (line, col) = self.open().buffer.line_col(&cursor);
            let text = self.open().buffer.line(line).to_string();
            if let Some((_, bytes)) = fenix_ccsds::hex::around(text.trim_end_matches(['\n', '\r']), col) {
                return Some((bytes, format!("{name}:{}", line + 1)));
            }
        }
        let clip = self.clipboard_text()?;
        fenix_ccsds::hex::parse(&clip).map(|b| (b, "the clipboard".to_string()))
    }

    /// `SPC k d`: the inspector on the bytes here.
    pub(crate) fn cmd_ccsds_decode(&mut self) {
        match self.ccsds_bytes_here() {
            Some((bytes, source)) => self.open_packet_page(bytes, source),
            None => self.set_error("no hex under the cursor, selected or on the clipboard"),
        }
    }

    /// The inspector on `bytes`.
    pub(crate) fn open_packet_page(&mut self, bytes: Vec<u8>, source: String) {
        let (key, _) = self.mib_key_here();
        let _ = self.mib_set(&key);
        let page = PacketPage::new(key, bytes, source);
        let id = self.open_page(PageModel::Packet(Box::new(page)));
        self.packet_redecode(id);
    }

    /// Decodes the page's bytes again, as it reads them now.
    pub(super) fn packet_redecode(&mut self, id: BufferId) {
        let Some(PageModel::Packet(p)) = self.pages.get(&id).map(|s| &s.model) else { return };
        let (key, bytes, reading) = (p.key.clone(), p.bytes.clone(), p.reading);
        let set = self.mib_set_ready(&key);
        let mission = self.mission(key.project.as_deref());
        let (root, _) = decode_bytes(&bytes, reading, set.as_deref(), &mission);
        if let Some(state) = self.pages.get_mut(&id) {
            state.stale = true;
            if let PageModel::Packet(p) = &mut state.model {
                p.root = root;
                if let Some(problem) = mission.problems.first() {
                    p.note = Some((format!("{problem} -- using the default"), true));
                }
            }
        }
    }

    pub(super) fn packet_page_action(&mut self, id: BufferId, action: packet_page::Action) {
        use packet_page::Action as A;
        let key = match self.pages.get(&id).map(|s| &s.model) {
            Some(PageModel::Packet(p)) => p.key.clone(),
            _ => return,
        };
        match action {
            A::None => {}
            A::Close => self.close_page(id),
            A::Redecode => self.packet_redecode(id),
            A::Copy(text, what) => {
                if let Some(clipboard) = &mut self.clipboard {
                    let _ = clipboard.set_text(text);
                }
                self.set_message(format!("copied the {what}"));
            }
            A::Settings => self.ccsds_open_settings(),
            A::Follow(link) => self.follow_link(key, link),
            A::FollowParameter(name) => match self.mib_set_ready(&key).and_then(|set| set.resolve(&name)) {
                Some(def) => self.follow_on_stream(def),
                None => self.set_error(format!("{name} isn't in the project's MIBs")),
            },
        }
    }

    /// `t` on a TM parameter: follows it through the recording or live
    /// source open most recently, shown on its Parameter tab.
    pub(crate) fn follow_on_stream(&mut self, def: DefRef) {
        let stream = self.buffers.mru().iter().copied().find(|b| matches!(self.pages.get(b).map(|s| &s.model), Some(PageModel::Stream(_))));
        let Some(stream) = stream else {
            self.set_error("no recording or live source open -- SPC k f or SPC k l, then t");
            return;
        };
        self.show_page(stream);
        self.stream_follow(stream, def);
    }

    /// The project's CCSDS settings.
    pub(crate) fn ccsds_open_settings(&mut self) {
        let scope = match self.settings_project() {
            Some((root, name)) => crate::settings_page::Scope::Project { root, name },
            None => crate::settings_page::Scope::You,
        };
        self.open_settings_page(scope, Some("ccsds.pus"));
    }

    /// Opens what a decoded field names.
    pub(crate) fn follow_link(&mut self, key: MibKey, link: Link) {
        let set = self.mib_set_ready(&key);
        let def = set.as_ref().and_then(|set| match &link {
            Link::Parameter(n) | Link::Telecommand(n) => set.resolve(n),
            Link::Spid(spid) => (0..set.roots().len()).find_map(|r| set.find(Kind::TmPacket, r, spid)),
            Link::Standard(..) => None,
        });
        match (def, link) {
            (Some(def), _) => self.open_mib_def(key, def),
            (None, Link::Standard(std, heading)) => self.open_standard(std, Some(heading)),
            (None, other) => self.set_message(format!("{other:?} isn't in this project's MIBs")),
        }
    }

    /// `K` on hex: a short reading of the packet or frame it spells.
    pub(super) fn ccsds_hex_hover(&mut self) -> Option<String> {
        if self.is_page_buffer(self.focused_buffer_id()) {
            return None;
        }
        let (line, col) = self.open().buffer.line_col(&self.cursor());
        let text = self.open().buffer.line(line).to_string();
        let text = text.trim_end_matches(['\n', '\r']);
        let (key, _) = self.mib_key_here();
        let mission = self.mission(key.project.as_deref());
        if let Some(hover) = time_hover(text, col, &mission) {
            return Some(hover);
        }
        let (_, bytes) = fenix_ccsds::hex::around(text, col)?;
        if bytes.len() < 6 {
            return None;
        }
        let set = if key.roots.is_empty() { None } else { self.mib_set(&key) };
        let (root, def) = decode_bytes(&bytes, DecodeAs::Auto, set.as_deref(), &mission);
        let walk = root.walk();
        let bad: Vec<String> = walk
            .iter()
            .filter_map(|(_, f)| match &f.check {
                Some(Check::Bad(m)) => Some(format!("{}: {m}", f.name)),
                _ => None,
            })
            .collect();
        let mut lines = vec![format!("{} · {} · {} bytes", root.name, root.value, bytes.len())];
        if let (Some(d), Some(set)) = (def, set.as_ref()) {
            let e = set.get(d);
            lines.push(format!("{} {} · {}", d.kind.tag(), e.name, if e.alias.is_empty() { &e.description } else { &e.alias }));
        }
        if let Some((_, t)) = walk.iter().find(|(_, f)| f.name.starts_with("time (")) {
            lines.push(t.value.clone());
        }
        lines.push(if bad.is_empty() { "every check passes".into() } else { bad.join("\n") });
        lines.push(String::new());
        lines.push("SPC k d opens the decode".into());
        Some(lines.join("\n"))
    }

    /// Opens a standard's PDF at `heading`, when the library has it.
    pub(crate) fn open_standard(&mut self, std: &str, heading: Option<&str>) {
        use crate::standards_page::{file_for, pdfs_in, STANDARDS};
        let Some(standard) = STANDARDS.iter().find(|s| s.id == std) else {
            self.set_message(format!("{std}{}", heading.map(|h| format!(" · {h}")).unwrap_or_default()));
            return;
        };
        let pdfs = self.config.ccsds_library.as_deref().map(pdfs_in).unwrap_or_default();
        match file_for(standard, &pdfs).cloned() {
            Some(path) => match heading {
                Some(h) => self.pdf_open_at_heading(&path, h),
                None => self.open_pdf_path(&path),
            },
            None => self.set_error(format!("{std} ({}) isn't in the standards folder -- SPC k ? says where to get it", standard.title)),
        }
    }

    /// `SPC k ?`: the standards library.
    pub(crate) fn cmd_ccsds_standards(&mut self) {
        let page = crate::standards_page::StandardsPage::new(self.config.ccsds_library.clone());
        self.open_page(PageModel::Standards(Box::new(page)));
    }

    pub(super) fn standards_action(&mut self, id: BufferId, action: crate::standards_page::Action) {
        use crate::standards_page::Action as A;
        match action {
            A::None => {}
            A::Close => self.close_page(id),
            A::Open(path) => self.open_pdf_path(&path),
            A::Url(url) => self.open_url(url),
            A::Settings => self.open_settings_page(crate::settings_page::Scope::You, Some("ccsds.library")),
            A::Search(query) => self.standards_search(id, query),
            A::OpenAt(path, page) => {
                self.open_pdf_path(&path);
                self.pdf_goto_page(page + 1);
            }
        }
    }

    /// Searches every standard the page's folder has, each document
    /// opened, searched and closed on the PDF worker; what's found goes
    /// to the page as each is read.
    fn standards_search(&mut self, id: BufferId, query: String) {
        // A search already under way for this page is dropped.
        let old: Vec<fenix_pdf::PdfDocKey> = self.standards_search.iter().filter(|(_, (page, _))| *page == id).map(|(k, _)| *k).collect();
        for key in old {
            self.standards_search.remove(&key);
            self.pdf_send(fenix_pdf::PdfRequest::Close { key });
        }
        let Some(PageModel::Standards(p)) = self.pages.get(&id).map(|s| &s.model) else { return };
        let files = p.searchable();
        if files.is_empty() {
            self.set_error("no standards to search -- f sets the folder they're in (ccsds.library)");
            return;
        }
        if let Some(PageModel::Standards(p)) = self.pages.get_mut(&id).map(|s| &mut s.model) {
            p.search_started(&query, files.len());
        }
        self.pdf_worker_ready();
        for (standard, path) in files {
            let key = fenix_pdf::PdfDocKey::new();
            self.standards_search.insert(key, (id, standard));
            self.pdf_send(fenix_pdf::PdfRequest::Open { key, path });
        }
        self.standards_query.insert(id, query);
    }

    /// A PDF worker reply for a standards search; whether it was one.
    pub(super) fn standards_search_reply(&mut self, response: &fenix_pdf::PdfResponse) -> bool {
        use fenix_pdf::PdfResponse as R;
        let key = match response {
            R::Opened { key, .. } | R::OpenFailed { key, .. } | R::SearchResults { key, .. } => *key,
            _ => return false,
        };
        let Some(&(page, standard)) = self.standards_search.get(&key) else { return false };
        let query = self.standards_query.get(&page).cloned().unwrap_or_default();
        let found = |app: &mut App, hits: Vec<(u32, String)>, done: bool| {
            if let Some(PageModel::Standards(p)) = app.pages.get_mut(&page).map(|s| &mut s.model) {
                p.found(standard, hits, done);
            }
            if let Some(state) = app.pages.get_mut(&page) {
                state.stale = true;
            }
        };
        match response {
            R::Opened { .. } => {
                // Smart case, as the reader's search.
                let match_case = query.chars().any(char::is_uppercase);
                self.pdf_send(fenix_pdf::PdfRequest::Search { key, request_id: 0, query, match_case, from_page: 0 });
            }
            R::OpenFailed { .. } => {
                self.standards_search.remove(&key);
                found(self, Vec::new(), true);
            }
            R::SearchResults { matches, done, .. } => {
                found(self, matches.iter().map(|m| (m.page_index, m.context.clone())).collect(), *done);
                if *done {
                    self.standards_search.remove(&key);
                    self.pdf_send(fenix_pdf::PdfRequest::Close { key });
                }
            }
            _ => {}
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        true
    }
}

/// `K` on an on-board time: the word under the cursor, when it's the
/// mission's time code -- `814B878A.8000` as the converter writes a CUC,
/// or its octets in hex -- read from the mission's epoch.
fn time_hover(line: &str, col: usize, m: &Mission) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let part = |c: char| c.is_ascii_hexdigit() || c == '.' || c == 'x' || c == 'X';
    if !chars.get(col).is_some_and(|c| part(*c)) {
        return None;
    }
    let (mut a, mut b) = (col, col);
    while a > 0 && part(chars[a - 1]) {
        a -= 1;
    }
    while b < chars.len() && part(chars[b]) {
        b += 1;
    }
    let word: String = chars[a..b].iter().collect();
    let word = word.trim_start_matches("0x").trim_start_matches("0X");
    let hex = |s: &str| (s.len() % 2 == 0).then(|| fenix_ccsds::hex::parse(s)).flatten();
    let bytes = match word.split_once('.') {
        Some((c, f)) => {
            let mut v = hex(c)?;
            v.extend(hex(f)?);
            v
        }
        None => hex(word)?,
    };
    let p = &m.profile;
    if bytes.len() != p.tm_time.len() {
        return None;
    }
    let d = fenix_ccsds::time::decode(&bytes, p.tm_time, p.epoch, &p.leap, 0)?;
    let utc = fenix_ccsds::time::tai_to_utc(d.tai, &p.leap);
    let correlated = if m.nominal_epoch.is_some() { " (correlated)" } else { "" };
    Some(format!(
        "{} · {}{correlated}\n{}\nSPC k T converts it",
        p.tm_time.label(),
        fenix_ccsds::time::format_utc(utc),
        fenix_ccsds::time::format_doy(utc)
    ))
}

/// Bytes decoded as `reading`, through the MIB when there is one: the
/// inspector's tree, and the definition it identified.
pub(crate) fn decode_bytes(bytes: &[u8], reading: DecodeAs, set: Option<&MibSet>, m: &Mission) -> (Field, Option<DefRef>) {
    use fenix_ccsds::{cfdp, coding, frames, time};
    let unreadable = |why: &str| (Field::new("not readable", 0, bytes.len() * 8, fenix_ccsds::field::hex(&bytes[..bytes.len().min(16)]), why).checked(Check::Bad(why.into())), None);
    let looks_like_packet = || fenix_ccsds::PrimaryHeader::decode(bytes).is_some_and(|h| h.plausible() && h.packet_len() <= bytes.len() + 2 && h.packet_len() + 64 >= bytes.len());
    let reading = match reading {
        DecodeAs::Auto if bytes.starts_with(&coding::ASM) => DecodeAs::Frame,
        DecodeAs::Auto if bytes.starts_with(&coding::CLTU_START) => DecodeAs::Cltu,
        DecodeAs::Auto if looks_like_packet() => DecodeAs::Packet,
        DecodeAs::Auto if bytes.len() == 4 => DecodeAs::Clcw,
        DecodeAs::Auto if bytes.len() == m.frames.length || bytes.len() == m.frames.length + 4 => DecodeAs::Frame,
        DecodeAs::Auto => DecodeAs::Packet,
        r => r,
    };
    match reading {
        DecodeAs::Packet | DecodeAs::Auto => match fenix_mib::packets::decode_packet(set, bytes, &m.profile, m.base) {
            Some(r) => r,
            None => unreadable("shorter than a packet header"),
        },
        DecodeAs::Clcw => match bytes.get(..4) {
            Some(b) => (frames::clcw_field(u32::from_be_bytes(b.try_into().unwrap_or([0; 4])), 0), None),
            None => unreadable("a CLCW is 4 octets"),
        },
        DecodeAs::Time => match time::decode(bytes, m.profile.tm_time, m.profile.epoch, &m.profile.leap, 0) {
            Some(d) => (d.field, None),
            None => unreadable("not a time in the project's format (ccsds.tm_time)"),
        },
        DecodeAs::Cfdp => match cfdp::decode(bytes) {
            Some(f) => (f, None),
            None => unreadable("shorter than a CFDP header"),
        },
        DecodeAs::Cltu => {
            let Some((cltu, sent)) = coding::cltu_decode(bytes, m.tc.bch_correct) else { return unreadable("no CLTU start sequence (EB90)") };
            let start = cltu.children.first().map(|f| f.bit / 8).unwrap_or(0);
            // Its layers side by side -- start sequence, TC frame, code
            // blocks, tail -- so the bytes take the frame's colour where
            // the frame is.
            let mut coding = cltu.children;
            let tail = coding.pop();
            let mut children = coding;
            // The randomizer ran over the frame before it was coded: it
            // comes off after the code blocks (the fill was never on it).
            let mut data = sent.clone();
            if m.tc.randomized {
                coding::derandomize(&mut data);
                children.insert(1, Field::new("de-randomized", (start + 2) * 8, 0, "", "pseudo-randomizer removed from the frame").linked(Link::Standard(coding::TC_STANDARD, "Pseudo-Randomizer")));
            }
            let mut def = None;
            if let Some((mut frame, info, bytes, problem)) = frames::tc_frame(&data, &m.tc) {
                if let Some(why) = problem {
                    frame.check = Some(Check::Bad(why));
                }
                let ctrl = bytes[0] & 0x10 != 0;
                if let Some(pkt) = bytes.get(info.data.clone()).filter(|_| !ctrl) {
                    if let Some((mut f, d)) = fenix_mib::packets::decode_packet(set, pkt, &m.profile, m.base) {
                        f.shift(info.data.start * 8);
                        frame.children.push(f);
                        def = d;
                    }
                }
                let fill = data.len() - bytes.len();
                if fill > 0 {
                    let all_fill = sent[bytes.len()..].iter().all(|b| *b == 0x55);
                    let f = Field::new("fill", bytes.len() * 8, fill * 8, format!("{fill} octets"), "after the frame, to fill the last code block")
                        .checked(if all_fill { Check::Ok("55".into()) } else { Check::Warn("not 0x55".into()) });
                    frame.children.push(f);
                }
                // The frame's octets are spread over the code blocks:
                // seven to a block, each block followed by its parity.
                frame.remap(&|o| coding::cltu_octet(start, o));
                let at = if m.tc.randomized { 2 } else { 1 };
                children.insert(at, frame);
            }
            children.extend(tail);
            (Field::group("CLTU", children), def)
        }
        DecodeAs::Frame => {
            let mut children = Vec::new();
            let mut at = 0;
            let mut frame = bytes.to_vec();
            if frame.starts_with(&coding::ASM) {
                children.push(Field::new("sync marker", 0, 32, "1ACFFC1D", "ASM").checked(Check::Ok("found".into())).linked(Link::Standard(coding::TM_STANDARD, "Attached Sync Marker")));
                frame.drain(..4);
                at = 4;
            }
            if m.frames.randomized {
                coding::derandomize(&mut frame);
                children.push(Field::new("de-randomized", at * 8, 0, "", "pseudo-randomizer removed").linked(Link::Standard(coding::TM_STANDARD, "Pseudo-Randomizer")));
            }
            if m.frames.rs_depth > 0 {
                if let Some((block, pad)) = fenix_ccsds::stream::rs_geometry(m.frames.length, m.frames.rs_depth) {
                    if frame.len() >= block {
                        frame.truncate(block);
                        let outcomes = fenix_ccsds::rs::decode_block(&mut frame, m.frames.rs_depth, pad);
                        let mut words = Vec::new();
                        for (w, o) in outcomes.iter().enumerate() {
                            let (text, check) = match o {
                                fenix_ccsds::rs::Outcome::Clean => ("no errors".to_string(), Check::Ok("clean".into())),
                                fenix_ccsds::rs::Outcome::Corrected(v) => (format!("{} symbol{} corrected", v.len(), if v.len() == 1 { "" } else { "s" }), Check::Warn(format!("corrected {}", v.len()))),
                                fenix_ccsds::rs::Outcome::Uncorrectable => ("beyond repair".to_string(), Check::Bad("more than 16 symbol errors".into())),
                            };
                            words.push(Field::new(format!("code word {}", w + 1), at * 8, 0, "", text).checked(check));
                        }
                        let mut rs = Field::group("Reed-Solomon (255,223)", words);
                        rs.bit = (at + m.frames.length) * 8;
                        rs.bits = 32 * m.frames.rs_depth * 8;
                        rs.link = Some(Link::Standard(fenix_ccsds::rs::STANDARD, "Reed-Solomon Coding"));
                        children.push(rs);
                        frame.truncate(m.frames.length);
                    }
                }
            }
            let Some((mut f, info)) = frames::decode(&frame, &m.frames) else { return unreadable("shorter than a frame header") };
            f.shift(at * 8);
            let mut def = None;
            if let (Some(fhp), Some(data)) = (info.first_header.filter(|h| *h != u16::MAX), frame.get(info.data.clone())) {
                let mut off = fhp as usize;
                while let Some(h) = data.get(off..).and_then(fenix_ccsds::PrimaryHeader::decode) {
                    let len = h.packet_len();
                    if !h.plausible() || off + len > data.len() {
                        break;
                    }
                    if let Some((mut p, d)) = fenix_mib::packets::decode_packet(set, &data[off..off + len], &m.profile, m.base) {
                        p.shift((at + info.data.start + off) * 8);
                        f.children.push(p);
                        def = def.or(d);
                    }
                    off += len;
                }
            }
            children.push(f);
            let title = children.last().map(|f| f.name.clone()).unwrap_or_default();
            (Field::group(title, children), def)
        }
    }
}

impl App {
    /// The text buffer and cursor a page's insert goes to.
    fn ccsds_origin(&self) -> Option<super::mib_host::MibOrigin> {
        let id = self.focused_buffer_id();
        let editable = |b: BufferId| !self.is_page_buffer(b) && self.buffers.get(b).is_some_and(|ob| ob.kind.tracks_unsaved_changes());
        let buffer = if editable(id) { Some(id) } else { self.buffers.mru().iter().copied().find(|&b| editable(b)) }?;
        let at = if buffer == id { self.cursor().char_idx } else { self.buffers.get(buffer).map(|ob| ob.cursor.char_idx).unwrap_or(0) };
        Some(super::mib_host::MibOrigin { buffer, range: at..at })
    }

    /// `SPC k T`: the time converter, on the time under the cursor when
    /// there's one in the project's format, else now.
    pub(crate) fn cmd_ccsds_time(&mut self) {
        let mission = self.mission(None);
        let p = mission.profile.clone();
        let mut tai = fenix_ccsds::time::utc_to_tai(chrono::Utc::now().naive_utc(), &p.leap);
        if !self.is_page_buffer(self.focused_buffer_id()) {
            let (line, col) = self.open().buffer.line_col(&self.cursor());
            let text = self.open().buffer.line(line).to_string();
            let run = fenix_ccsds::hex::around(text.trim_end_matches(['\n', '\r']), col).map(|(_, b)| b);
            if let Some(d) = run.and_then(|b| fenix_ccsds::time::decode(&b, p.tm_time, p.epoch, &p.leap, 0)) {
                tai = d.tai;
            }
        }
        let origin = self.ccsds_origin();
        let mut page = TimePage::new(p, tai);
        page.nominal = mission.nominal_epoch;
        let id = self.open_page(PageModel::Time(Box::new(page)));
        if let Some(o) = origin {
            self.mib_origins.insert(id, o);
        }
    }

    /// The time now as the mission writes it on board, for the modeline,
    /// when the project turns `ccsds.clock` on. Worked out at most four
    /// times a second.
    pub(crate) fn mission_clock_text(&mut self) -> Option<String> {
        let on = match self.project_settings.as_ref().and_then(|(_, p)| p.get("ccsds.clock")) {
            Some(fenix_config::Value::Bool(b)) => *b,
            _ => self.config.ccsds_clock.unwrap_or(false),
        };
        if !on {
            return None;
        }
        let tick = chrono::Utc::now().timestamp_millis() / 250;
        if let Some((at, text)) = &self.mission_clock {
            if *at == tick {
                return Some(text.clone());
            }
        }
        let m = self.mission(None);
        let tai = fenix_ccsds::time::utc_to_tai(chrono::Utc::now().naive_utc(), &m.profile.leap);
        let text = format!("OBT {}", time_page::mission_code(tai, &m.profile));
        self.mission_clock = Some((tick, text.clone()));
        Some(text)
    }

    pub(super) fn time_page_action(&mut self, id: BufferId, action: time_page::Action) {
        match action {
            time_page::Action::None => {}
            time_page::Action::Close => {
                self.mib_origins.remove(&id);
                self.close_page(id);
            }
            time_page::Action::Copy(text) => {
                if let Some(clipboard) = &mut self.clipboard {
                    let _ = clipboard.set_text(text.clone());
                }
                self.set_message(format!("copied {text}"));
            }
            time_page::Action::Insert(text) => {
                let Some(o) = self.mib_origins.get(&id).cloned() else {
                    self.set_error("open the file the time goes in first");
                    return;
                };
                let Some(ob) = self.buffers.get_mut(o.buffer) else { return };
                let mut cursor = Cursor { char_idx: o.range.start.min(ob.buffer.len_chars()), sticky_col: 0 };
                ob.buffer.insert_str(&mut cursor, &text);
                if let Some(o) = self.mib_origins.get_mut(&id) {
                    o.range = cursor.char_idx..cursor.char_idx;
                }
                self.set_message(format!("inserted {text}"));
            }
        }
    }

    /// `SPC f x`: the focused file (or the explorer's) as hex.
    /// A file no text buffer can hold: NULs, or bytes that aren't UTF-8,
    /// in its first 8 KB. Opening one shows it in the hex view.
    pub(crate) fn looks_binary(path: &Path) -> bool {
        use std::io::Read;
        let Ok(f) = std::fs::File::open(path) else { return false };
        let mut head = Vec::new();
        if f.take(8192).read_to_end(&mut head).is_err() {
            return false;
        }
        head.contains(&0) || std::str::from_utf8(&head).is_err_and(|e| e.error_len().is_some())
    }

    /// The file the focused page shows -- a hex view's, a recording's --
    /// so the project (and its MIBs) follow it as they follow a buffer.
    pub(crate) fn page_file(&self) -> Option<PathBuf> {
        let id = self.focused_buffer_id();
        match self.pages.get(&id).map(|s| &s.model) {
            Some(PageModel::Hex(p)) => Some(p.path.clone()),
            Some(PageModel::Stream(_)) => match self.stream_sources.get(&id) {
                Some(super::stream_host::StreamSource::File(path, _)) => Some(path.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// The project a MIB or mission page was opened in.
    pub(crate) fn page_project(&self) -> Option<PathBuf> {
        let key = match self.pages.get(&self.focused_buffer_id()).map(|s| &s.model)? {
            PageModel::Stream(p) => &p.key,
            PageModel::Packet(p) => &p.key,
            PageModel::Mib(p) => &p.key,
            PageModel::MibDef(p) => &p.key,
            PageModel::MibForm(p) => &p.key,
            _ => return None,
        };
        key.project.clone()
    }

    /// The file a mission command acts on: the explorer's selection, the
    /// file a hex view shows, or the open buffer's.
    pub(crate) fn ccsds_file_here(&mut self) -> Option<PathBuf> {
        if self.main_view == MainView::Explorer {
            return self.explorer_selected_path();
        }
        self.page_file().or_else(|| self.open().buffer.path().map(Path::to_path_buf))
    }

    pub(crate) fn cmd_hex_view(&mut self) {
        let Some(path) = self.ccsds_file_here().filter(|p| p.is_file()) else {
            self.set_error("no file here to show as hex");
            return;
        };
        self.open_hex_view(path);
    }

    pub(crate) fn open_hex_view(&mut self, path: PathBuf) {
        use std::io::Read;
        let total = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let mut bytes = Vec::new();
        match std::fs::File::open(&path) {
            Ok(f) => {
                if let Err(e) = f.take(hex_page::LIMIT).read_to_end(&mut bytes) {
                    self.set_error(format!("couldn't read {}: {e}", path.display()));
                    return;
                }
            }
            Err(e) => {
                self.set_error(format!("couldn't open {}: {e}", path.display()));
                return;
            }
        }
        self.main_view = MainView::Editor;
        let over = (total > hex_page::LIMIT).then_some(total);
        self.open_page(PageModel::Hex(Box::new(HexPage::new(path, bytes, over))));
    }

    pub(super) fn hex_page_action(&mut self, id: BufferId, action: hex_page::Action) {
        match action {
            hex_page::Action::None => {}
            hex_page::Action::Close => self.close_page(id),
            hex_page::Action::Decode(at) => {
                let Some(PageModel::Hex(p)) = self.pages.get(&id).map(|s| &s.model) else { return };
                let rest = &p.bytes[at.min(p.bytes.len())..];
                let mission = self.mission(None);
                let len = if rest.starts_with(&fenix_ccsds::coding::ASM) {
                    4 + fenix_ccsds::stream::rs_geometry(mission.frames.length, mission.frames.rs_depth).map(|(n, _)| n).unwrap_or(mission.frames.length)
                } else {
                    match fenix_ccsds::PrimaryHeader::decode(rest) {
                        Some(h) if h.plausible() => h.packet_len(),
                        _ => 64,
                    }
                };
                let bytes = rest[..len.min(rest.len())].to_vec();
                let source = format!("{} @0x{at:X}", p.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                self.open_packet_page(bytes, source);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_ccsds::coding;

    /// ZTC08101 (line 3, AUTO, seq 7) in a Type-A TC frame with a segment
    /// header and an FECF, in a CLTU.
    fn cltu() -> Vec<u8> {
        coding::cltu_encode(&frame())
    }

    fn frame() -> Vec<u8> {
        let tc = [0x1B, 0xF2, 0xC0, 0x07, 0x00, 0x09, 0x29, 0x08, 0x01, 0x00, 0x00, 0x0C, 0x03, 0x02, 0x3F, 0xA3];
        let len = 5 + 1 + tc.len() + 2;
        let mut f = vec![0x00, 0xA5, ((len - 1) >> 8) as u8, (len - 1) as u8, 42, 0xC0];
        f.extend_from_slice(&tc);
        let crc = fenix_ccsds::crc::ccitt16(&f);
        f.extend_from_slice(&crc.to_be_bytes());
        f
    }

    #[test]
    fn k_on_an_on_board_time_says_when_it_was() {
        let m = App::with_file(None).mission(None);
        let line = "  at 814B878A.8000 the heater came on (814B878A8000, 0x814B878A8000)";
        for word in ["814B878A.8000", "814B878A8000,", "0x814B878A8000"] {
            let col = line.find(word).unwrap() + 3;
            let hover = time_hover(line, col, &m).unwrap_or_else(|| panic!("{word}"));
            assert!(hover.starts_with("CUC 4+2 · 2026-09-27 14:32:05.500") && hover.contains("2026-270T14:32:05.500Z"), "{hover}");
        }
        assert!(time_hover(line, line.find("heater").unwrap(), &m).is_none());
        assert!(time_hover("0B F2 C1 23", 1, &m).is_none(), "not the time code's length");
    }

    #[test]
    fn t_on_a_tm_parameter_follows_it_on_the_recording_open() {
        let mut app = App::with_file(None);
        let set = MibSet::load(vec![fenix_mib::MibRoot { label: "OPS".into(), path: std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dev/ccsds-sim/mission/ops") }], None);
        let def = set.find(Kind::TmParam, 0, "NTH00123").unwrap();
        app.follow_on_stream(def);
        assert!(app.status_message.as_ref().is_some_and(|m| m.text.starts_with("no recording or live source open")));
    }

    #[test]
    fn a_time_correlation_moves_every_decode_and_the_clock_shows_on_board_time() {
        let mut app = App::with_file(None);
        assert!(app.mission(None).nominal_epoch.is_none());
        assert!(app.mission_clock_text().is_none(), "off by default");
        app.config.ccsds_time_correlation = Some("814B878A.8000 = 2026-09-27 14:31:55.5".into());
        let m = app.mission(None);
        assert!(m.nominal_epoch.is_some() && m.problems.is_empty(), "{:?}", m.problems);
        // The example packet's time now reads 10 s earlier.
        let tm = [0x0B, 0xF2, 0xC1, 0x23, 0x00, 0x16, 0x20, 0x03, 0x19, 0x00, 0x42, 0x00, 0x00, 0x81, 0x4B, 0x87, 0x8A, 0x80, 0x00, 0x00, 0x01, 0x01, 0x0B, 0xB8, 0x0A, 0x28, 0x02, 0x60, 0xE5];
        let (root, _) = decode_bytes(&tm, DecodeAs::Packet, None, &m);
        assert_eq!(root.find("time (CUC 4+2)").map(|f| f.value.clone()).as_deref(), Some("2026-09-27 14:31:55.500 UTC"));
        app.config.ccsds_time_correlation = Some("soon = then".into());
        assert!(app.mission(None).problems.iter().any(|p| p.starts_with("ccsds.time_correlation")));
        app.config.ccsds_clock = Some(true);
        let clock = app.mission_clock_text().unwrap();
        assert!(clock.starts_with("OBT ") && clock.len() == "OBT 814B878A.8000".len(), "{clock}");
    }

    #[test]
    fn a_randomized_cltu_with_a_wrong_bit_still_gives_its_telecommand() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dev/ccsds-sim/mission/ops");
        let set = MibSet::load(vec![fenix_mib::MibRoot { label: "OPS".into(), path: dir }], None);
        let mut m = App::with_file(None).mission(None);
        m.tc.randomized = true;
        let mut f = frame();
        coding::derandomize(&mut f); // randomizes: its own inverse
        let mut bytes = coding::cltu_encode(&f);
        bytes[2 + 8 + 3] ^= 0x10; // one bit in code block 2
        let (root, def) = decode_bytes(&bytes, DecodeAs::Cltu, Some(&set), &m);
        assert_eq!(def.map(|d| set.get(d).name.clone()).as_deref(), Some("ZTC08101"), "{root:#?}");
        assert!(root.find("de-randomized").is_some());
        let block = root.find("code block 2").unwrap();
        assert_eq!(block.check, Some(Check::Warn("1 bit corrected".into())));
        assert!(!root.any_bad(), "{root:#?}");
        assert!(root.find("fill").and_then(|f| f.check.clone()) == Some(Check::Ok("55".into())), "fill is checked as sent");
        // Detecting only: the same block fails.
        m.tc.bch_correct = false;
        let (root, _) = decode_bytes(&bytes, DecodeAs::Cltu, Some(&set), &m);
        assert!(root.find("code block 2").and_then(|b| b.check.clone()).is_some_and(|c| c.is_bad()));
    }

    #[test]
    fn a_cltu_is_decomposed_down_to_its_telecommand_on_the_octets_it_came_in() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dev/ccsds-sim/mission/ops");
        let set = MibSet::load(vec![fenix_mib::MibRoot { label: "OPS".into(), path: dir }], None);
        let m = App::with_file(None).mission(None);
        let bytes = cltu();
        let (root, def) = decode_bytes(&bytes, DecodeAs::Auto, Some(&set), &m);
        assert_eq!(def.map(|d| set.get(d).name.clone()).as_deref(), Some("ZTC08101"));
        assert!(!root.any_bad(), "{root:#?}");
        let f = |name: &str| root.find(name).unwrap_or_else(|| panic!("no {name} in {root:#?}"));
        assert!(f("tail sequence").check == Some(Check::Ok("found".into())));
        assert!(f("frame length").check.as_ref().is_some_and(|c| !c.is_bad()), "the fill isn't counted in the frame");
        assert_eq!(f("fill").bits, 4 * 8, "24 octets of frame in 4 blocks of 7: 4 of fill");
        // The TC frame's first octet is the first code block's first data
        // octet; the packet starts at frame octet 6, which is CLTU octet 8,
        // and frame octet 7 is past the first block's parity: CLTU octet 10.
        assert_eq!(f("SCID").bit / 8, 2);
        assert_eq!(f("segment header").bit / 8, 7);
        let packet = root.walk().into_iter().map(|(_, x)| x).find(|x| x.name.starts_with("TC(8,1)") || x.name.contains("ZTC08101")).map(|x| x.bit / 8);
        assert_eq!(packet, Some(8), "{root:#?}");
        assert!(f("FECF").check == Some(Check::Ok("matches".into())));
        // Two flipped bits in the second code block: beyond correction.
        let mut broken = bytes.clone();
        broken[2 + 8 + 1] ^= 0x41;
        let (root, _) = decode_bytes(&broken, DecodeAs::Auto, Some(&set), &m);
        assert!(root.find("code block 2").and_then(|b| b.check.clone()).is_some_and(|c| c.is_bad()));
    }
}
