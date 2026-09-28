//! `SPC k T`: the time converter. One instant, shown every way a mission
//! writes it -- its on-board time code (the project's `ccsds.tm_time`
//! from `ccsds.epoch`), seconds since that epoch, UTC, TAI, GPS, CDS, day
//! of year -- and typed into any of them to update the rest. `i` inserts
//! the on-board time where the converter was opened.

use chrono::{Duration, NaiveDate};
use fenix_ccsds::field::hex_tight;
use fenix_ccsds::pus::Profile;
use fenix_ccsds::time::{self, Tai, TimeFormat};

use crate::page::{fit, fit_tail, frame, Grid, Key, Page, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Mission,
    Seconds,
    Utc,
    Tai,
    Gps,
    Cds,
    Doy,
}

const ROWS: [Row; 7] = [Row::Mission, Row::Seconds, Row::Utc, Row::Tai, Row::Gps, Row::Cds, Row::Doy];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    Copy(String),
    Insert(String),
}

pub struct TimePage {
    profile: Profile,
    pub tai: Tai,
    sel: usize,
    editing: Option<String>,
    pub note: Option<(String, bool)>,
}

fn base_1958() -> chrono::NaiveDateTime {
    NaiveDate::from_ymd_opt(1958, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap()
}

/// The on-board time code, CUC as `coarse.fine` hex.
pub fn mission_code(tai: Tai, p: &Profile) -> String {
    let bytes = time::encode(tai, p.tm_time, p.epoch);
    match p.tm_time {
        TimeFormat::Cuc { coarse, fine, pfield } => {
            let pl = if pfield { if coarse > 4 || fine > 3 { 2 } else { 1 } } else { 0 };
            let (pf, rest) = bytes.split_at(pl.min(bytes.len()));
            let (c, f) = rest.split_at((coarse as usize).min(rest.len()));
            let mut s = String::new();
            if !pf.is_empty() {
                s.push_str(&format!("[{}] ", hex_tight(pf)));
            }
            s.push_str(&hex_tight(c));
            if !f.is_empty() {
                s.push('.');
                s.push_str(&hex_tight(f));
            }
            s
        }
        _ => hex_tight(&bytes),
    }
}

impl TimePage {
    pub fn new(profile: Profile, tai: Tai) -> Self {
        TimePage { profile, tai, sel: 0, editing: None, note: None }
    }

    pub fn typing(&self) -> bool {
        self.editing.is_some()
    }

    pub fn paste(&mut self, text: &str) {
        if let Some(e) = &mut self.editing {
            e.push_str(text.trim());
        }
    }

    fn value(&self, row: Row) -> String {
        let p = &self.profile;
        let t = self.tai;
        match row {
            Row::Mission => mission_code(t, p),
            Row::Seconds => {
                let s = t.0 - p.epoch.tai.0;
                let s = format!("{s:.6}");
                s.trim_end_matches('0').trim_end_matches('.').to_string()
            }
            Row::Utc => time::format_utc(time::tai_to_utc(t, &p.leap)),
            Row::Tai => (base_1958() + Duration::microseconds((t.0 * 1e6).round() as i64)).format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
            Row::Gps => {
                let g = time::tai_to_gps(t);
                format!("week {} · {} s", (g / 604800.0).floor(), trim(g.rem_euclid(604800.0)))
            }
            Row::Cds => {
                let bytes = time::encode(t, TimeFormat::Cds { days24: false, submilli: 0, pfield: false }, p.epoch);
                let days = u16::from_be_bytes([bytes[0], bytes[1]]);
                let ms = u32::from_be_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]);
                format!("{days} · {ms} ms")
            }
            Row::Doy => time::format_doy(time::tai_to_utc(t, &p.leap)),
        }
    }

    fn label(&self, row: Row) -> String {
        match row {
            Row::Mission => format!("on board ({})", self.profile.tm_time.label()),
            Row::Seconds => "seconds since epoch".into(),
            Row::Utc => "UTC".into(),
            Row::Tai => "TAI".into(),
            Row::Gps => "GPS".into(),
            Row::Cds => "CDS (16-bit days)".into(),
            Row::Doy => "day of year (UTC)".into(),
        }
    }

    /// Sets the time from `text` typed into `row`.
    fn set(&mut self, row: Row, text: &str) -> Result<(), String> {
        let p = &self.profile;
        let text = text.trim();
        let t = match row {
            Row::Mission => {
                let clean: String = text.chars().filter(|c| c.is_ascii_hexdigit()).collect();
                let bytes = fenix_ccsds::hex::parse(&clean).ok_or("hex, as the on-board time is written")?;
                time::decode(&bytes, p.tm_time, p.epoch, &p.leap, 0).ok_or(format!("not a {}", p.tm_time.label()))?.tai
            }
            Row::Seconds => Tai(p.epoch.tai.0 + text.parse::<f64>().map_err(|_| "a number of seconds")?),
            Row::Utc | Row::Doy => time::utc_to_tai(time::parse_utc(text).ok_or("a date and time, like 2026-09-27 14:32:05.5")?, &p.leap),
            Row::Tai => {
                let at = time::parse_utc(text).ok_or("a date and time")?;
                Tai((at - base_1958()).num_microseconds().unwrap_or(0) as f64 / 1e6)
            }
            Row::Gps => {
                let nums: Vec<f64> = text.split(|c: char| !c.is_ascii_digit() && c != '.').filter_map(|w| w.parse().ok()).collect();
                let secs = match nums.as_slice() {
                    [week, s] => week * 604800.0 + s,
                    [s] => *s,
                    _ => return Err("week and seconds, or seconds".into()),
                };
                time::gps_to_tai(secs)
            }
            Row::Cds => {
                let nums: Vec<f64> = text.split(|c: char| !c.is_ascii_digit()).filter_map(|w| w.parse().ok()).collect();
                let [days, ms] = nums[..] else { return Err("days and milliseconds".into()) };
                Tai(p.epoch.tai.0 + days * 86400.0 + ms / 1000.0)
            }
        };
        self.tai = t;
        Ok(())
    }

    pub fn key(&mut self, key: Key) -> Action {
        self.note = None;
        if let Some(text) = &mut self.editing {
            match key {
                Key::Escape => self.editing = None,
                Key::Enter => {
                    let text = std::mem::take(text);
                    self.editing = None;
                    if let Err(e) = self.set(ROWS[self.sel], &text) {
                        self.note = Some((e, true));
                    }
                }
                Key::Backspace => {
                    text.pop();
                }
                Key::Char(c) => text.push(c),
                Key::Space => text.push(' '),
                _ => {}
            }
            return Action::None;
        }
        match key {
            Key::Char('q') | Key::Escape => return Action::Close,
            Key::Down | Key::Char('j') | Key::Tab => self.sel = (self.sel + 1).min(ROWS.len() - 1),
            Key::Up | Key::Char('k') | Key::BackTab => self.sel = self.sel.saturating_sub(1),
            Key::Enter | Key::Char('c') => self.editing = Some(String::new()),
            Key::Char('e') => self.editing = Some(self.value(ROWS[self.sel])),
            Key::Char('n') => self.tai = time::utc_to_tai(chrono::Utc::now().naive_utc(), &self.profile.leap),
            Key::Char('y') => return Action::Copy(self.value(ROWS[self.sel])),
            Key::Char('i') => return Action::Insert(hex_tight(&time::encode(self.tai, self.profile.tm_time, self.profile.epoch))),
            _ => {}
        }
        Action::None
    }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn layout(p: &TimePage, cols: usize) -> Page {
    let (left, width) = frame(cols, 110);
    let mut g = Grid::new();
    g.put(1, left, "Time", Role::Title);
    let epoch = if p.profile.epoch.level1 { "1958 TAI (level 1)".to_string() } else { format!("{} TAI", (base_1958() + Duration::seconds(p.profile.epoch.tai.0 as i64)).format("%Y-%m-%d %H:%M:%S")) };
    let info = format!("{} · epoch {epoch} · TAI - UTC {} s", p.profile.tm_time.label(), p.profile.leap.offset(time::tai_to_utc(p.tai, &p.profile.leap).date()));
    g.put(1, (left + width).saturating_sub(info.chars().count()), &info, Role::Muted);
    let mut y = 3;
    if let Some((text, bad)) = &p.note {
        g.put(2, left, &fit(text, width), if *bad { Role::Bad } else { Role::Good });
    }
    for (i, row) in ROWS.iter().enumerate() {
        g.put(y, left, &p.label(*row), Role::Muted);
        let on = i == p.sel;
        let x = left + 24;
        match (&p.editing, on) {
            (Some(text), true) => {
                let e = g.put(y, x, &fit_tail(&format!("{text}▏"), width - 26), Role::Title);
                g.panels.push((y, x..e.max(x + 30)));
            }
            _ => {
                g.put(y, x, &p.value(*row), if on { Role::Title } else { Role::Text });
            }
        }
        if on {
            g.focus(y, left..left + width);
        }
        y += 1;
    }
    let keys: &[(&str, &str)] = if p.editing.is_some() {
        &[("Enter", "convert"), ("Esc", "leave it")]
    } else {
        &[("Enter", "type a time here"), ("e", "edit this one"), ("n", "now"), ("y", "copy"), ("i", "insert the on-board time"), ("q", "close")]
    };
    g.keys(left, width, keys);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> TimePage {
        let p = Profile::default();
        let t = time::utc_to_tai(time::parse_utc("2026-09-27 14:32:05.5").unwrap(), &p.leap);
        TimePage::new(p, t)
    }

    #[test]
    fn every_form_of_the_example_time() {
        let text = layout(&page(), 120).text;
        for want in ["814B878A.8000", "2169210762.5", "2026-09-27 14:32:05.500", "2026-09-27 14:32:42.500", "week 2438 · 52343.5 s", "25106 · 52362500 ms", "2026-270T14:32:05.500Z"] {
            assert!(text.contains(want), "{want}: {text}");
        }
    }

    #[test]
    fn typing_into_any_row_moves_the_others() {
        let mut p = page();
        p.key(Key::Char('j'));
        p.key(Key::Char('j'));
        p.key(Key::Enter);
        for c in "2000-01-01T12:00:00".chars() {
            p.key(Key::Char(c));
        }
        p.key(Key::Enter);
        assert!(layout(&p, 120).text.contains("2000-01-01 12:00:32.000"), "TAI is 32 s ahead in 2000");
        p.key(Key::Char('k'));
        p.key(Key::Char('k'));
        p.key(Key::Enter);
        for c in "814B878A8000".chars() {
            p.key(Key::Char(c));
        }
        p.key(Key::Enter);
        assert_eq!(p.key(Key::Char('i')), Action::Insert("814B878A8000".into()));
        p.key(Key::Enter);
        for c in "zz".chars() {
            p.key(Key::Char(c));
        }
        p.key(Key::Enter);
        assert!(p.note.as_ref().is_some_and(|(_, bad)| *bad));
    }
}
