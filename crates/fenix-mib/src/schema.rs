//! SCOS-2000 MIB table column layouts, per the Database Import ICD, for
//! each of its main issues -- 4.x, 5.x, 6.x, 6.9 and 7.x (7.2's layout,
//! transcribed from the reference implementation this crate ports,
//! orbit-emacs's `mod-mib--schemas-7.2`).
//!
//! The ICD only ever adds: a new issue brings new tables, and new
//! columns at the end of a row, so an older MIB's lines are a prefix of
//! the newer layout. Each table here is written in its newest layout,
//! and marked with the issue it first appeared in and the issue each of
//! its later columns did; an issue's layout is what was there by then.
//! The per-issue markers are Fenix's best reading of the ICD's change
//! history, not a copy of the document: a mission whose files differ --
//! an issue's quirk, or tables and columns of its own -- says so with
//! `Schema::columns` (`mib.columns` in the settings), which takes the
//! place of the built-in layout for that table.

use std::path::Path;

/// An issue of the SCOS-2000 Database Import ICD, oldest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Icd {
    V4,
    V5,
    V6,
    V6_9,
    V7,
}

impl Icd {
    pub const ALL: [Icd; 5] = [Icd::V4, Icd::V5, Icd::V6, Icd::V6_9, Icd::V7];
    pub const LATEST: Icd = Icd::V7;

    /// How the settings and the MIB page write it.
    pub fn label(self) -> &'static str {
        match self {
            Icd::V4 => "4.x",
            Icd::V5 => "5.x",
            Icd::V6 => "6.x",
            Icd::V6_9 => "6.9",
            Icd::V7 => "7.x",
        }
    }

    /// An issue as someone writes it: `7.2`, `7`, `7.x`, `ICD 6.9`,
    /// `6.3`, `5`, ... -- the issue it falls under, or `None`.
    pub fn parse(text: &str) -> Option<Icd> {
        let t = text.trim().trim_start_matches(|c: char| c.is_ascii_alphabetic() || c == ' ' || c == '-' || c == '_');
        let (major, minor) = t.split_once('.').unwrap_or((t, ""));
        let minor: u32 = minor.trim_end_matches(['x', 'X']).parse().unwrap_or(0);
        match major.trim().parse::<u32>().ok()? {
            4 => Some(Icd::V4),
            5 => Some(Icd::V5),
            6 if minor >= 9 => Some(Icd::V6_9),
            6 => Some(Icd::V6),
            7.. => Some(Icd::V7),
            _ => None,
        }
    }
}

impl std::fmt::Display for Icd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// One table of the ICD: its lower-case name (the `.dat` file's base
/// name), what it holds, the issue it first appeared in, and its columns
/// in file order, each with the issue it was added in when that's later
/// than the table's own. A column's issue holds for the ones after it
/// until another says otherwise -- the ICD appends.
struct Table {
    name: &'static str,
    title: &'static str,
    since: Icd,
    columns: &'static [&'static str],
    added: &'static [(&'static str, Icd)],
}

const fn t(name: &'static str, title: &'static str, since: Icd, columns: &'static [&'static str], added: &'static [(&'static str, Icd)]) -> Table {
    Table { name, title, since, columns, added }
}

use Icd::{V4, V5, V6, V6_9, V7};

const TABLES: &[Table] = &[
    // Version.
    t("vdf", "database version", V4, &["VDF_NAME", "VDF_COMMENT", "VDF_DOMAINID", "VDF_RELEASE", "VDF_ISSUE"], &[("VDF_DOMAINID", V6)]),
    // Monitoring: packets.
    t("pid", "TM packet identification", V4, &["PID_TYPE", "PID_STYPE", "PID_APID", "PID_PI1_VAL", "PID_PI2_VAL", "PID_SPID", "PID_DESCR", "PID_UNIT", "PID_TPSD", "PID_DFHSIZE", "PID_TIME", "PID_INTER", "PID_VALID", "PID_CHECK", "PID_EVENT", "PID_EVID"], &[("PID_CHECK", V5), ("PID_EVENT", V6)]),
    t("pic", "TM packet identification criteria", V4, &["PIC_TYPE", "PIC_STYPE", "PIC_PI1_OFF", "PIC_PI1_WID", "PIC_PI2_OFF", "PIC_PI2_WID", "PIC_APID"], &[("PIC_APID", V6_9)]),
    t("tpcf", "TM packet characteristics", V4, &["TPCF_SPID", "TPCF_NAME", "TPCF_SIZE"], &[("TPCF_SIZE", V5)]),
    t("plf", "parameters in fixed TM packets", V4, &["PLF_NAME", "PLF_SPID", "PLF_OFFBY", "PLF_OFFBI", "PLF_NBOCC", "PLF_LGOCC", "PLF_TIME", "PLF_TDOCC"], &[]),
    t("vpd", "variable TM packet contents", V4, &["VPD_TPSD", "VPD_POS", "VPD_NAME", "VPD_GRPSIZE", "VPD_FIXREP", "VPD_CHOICE", "VPD_PIDREF", "VPD_DISDESC", "VPD_WIDTH", "VPD_JUSTIFY", "VPD_NEWLINE", "VPD_DCHAR", "VPD_FORM", "VPD_OFFSET"], &[("VPD_OFFSET", V6_9)]),
    t("grp", "parameter and packet groups", V4, &["GRP_NAME", "GRP_DESCR", "GRP_GTYPE"], &[]),
    t("grpa", "parameters in groups", V4, &["GRPA_GNAME", "GRPA_PANAME"], &[]),
    t("grpk", "packets in groups", V4, &["GRPK_GNAME", "GRPK_PKSPID"], &[]),
    // Monitoring: parameters.
    t("pcf", "TM parameter characteristics", V4, &["PCF_NAME", "PCF_DESCR", "PCF_PID", "PCF_UNIT", "PCF_PTC", "PCF_PFC", "PCF_WIDTH", "PCF_VALID", "PCF_RELATED", "PCF_CATEG", "PCF_NATUR", "PCF_CURTX", "PCF_INTER", "PCF_USCON", "PCF_DECIM", "PCF_PARVAL", "PCF_SUBSYS", "PCF_VALPAR", "PCF_SPTYPE", "PCF_CORR", "PCF_OBTID", "PCF_DARC", "PCF_ENDIAN", "PCF_DESCR2"], &[("PCF_CORR", V5), ("PCF_DARC", V6), ("PCF_ENDIAN", V7)]),
    t("cur", "calibration selection", V4, &["CUR_PNAME", "CUR_POS", "CUR_RLCHK", "CUR_VALPAR", "CUR_SELECT"], &[("CUR_SELECT", V6)]),
    t("caf", "TM numeric calibrations", V4, &["CAF_NUMBR", "CAF_DESCR", "CAF_ENGFMT", "CAF_RAWFMT", "CAF_RADIX", "CAF_UNIT", "CAF_NCURVE", "CAF_INTER"], &[("CAF_INTER", V6)]),
    t("cap", "TM numeric calibration points", V4, &["CAP_NUMBR", "CAP_XVALS", "CAP_YVALS"], &[]),
    t("txf", "TM text calibrations", V4, &["TXF_NUMBR", "TXF_DESCR", "TXF_RAWFMT", "TXF_NALIAS"], &[]),
    t("txp", "TM text calibration ranges", V4, &["TXP_NUMBR", "TXP_FROM", "TXP_TO", "TXP_ALTXT"], &[]),
    t("mcf", "polynomial calibrations", V4, &["MCF_IDENT", "MCF_DESCR", "MCF_POL1", "MCF_POL2", "MCF_POL3", "MCF_POL4", "MCF_POL5"], &[]),
    t("lgf", "logarithmic calibrations", V4, &["LGF_IDENT", "LGF_DESCR", "LGF_POL1", "LGF_POL2", "LGF_POL3", "LGF_POL4", "LGF_POL5"], &[]),
    t("ocf", "monitoring checks", V4, &["OCF_NAME", "OCF_NBCHCK", "OCF_NBOOL", "OCF_INTER", "OCF_CODIN"], &[]),
    t("ocp", "monitoring check limits", V4, &["OCP_NAME", "OCP_POS", "OCP_TYPE", "OCP_LVALU", "OCP_HVALU", "OCP_RLCHK", "OCP_VALPAR"], &[]),
    // Displays.
    t("dpf", "alphanumeric displays", V4, &["DPF_NUMBE", "DPF_TYPE", "DPF_HEAD"], &[]),
    t("dpc", "alphanumeric display parameters", V4, &["DPC_NUMBE", "DPC_NAME", "DPC_FLDN", "DPC_COMM", "DPC_MODE", "DPC_FORM", "DPC_TEXT"], &[]),
    t("gpf", "graphic displays", V4, &["GPF_NUMBE", "GPF_TYPE", "GPF_HEAD", "GPF_SCROL", "GPF_HCOPY", "GPF_DAYS", "GPF_HOURS", "GPF_MINUT", "GPF_AXCLR", "GPF_XTICK", "GPF_YTICK", "GPF_XGRID", "GPF_YGRID", "GPF_UPUN"], &[]),
    t("gpc", "graphic display parameters", V4, &["GPC_NUMBE", "GPC_POS", "GPC_WHERE", "GPC_NAME", "GPC_RAW", "GPC_MINIM", "GPC_MAXIM", "GPC_PRCLR", "GPC_SYMBO", "GPC_LINE", "GPC_DOMAIN"], &[]),
    t("spf", "scrolling displays", V4, &["SPF_NUMBE", "SPF_HEAD", "SPF_NPAR", "SPF_UPUN"], &[]),
    t("spc", "scrolling display parameters", V4, &["SPC_NUMBE", "SPC_POS", "SPC_NAME", "SPC_UPDT", "SPC_MODE", "SPC_FORM", "SPC_BACK", "SPC_FORE"], &[]),
    t("ppf", "printout displays", V4, &["PPF_NUMBE", "PPF_HEAD", "PPF_NBPR"], &[]),
    t("ppc", "printout display parameters", V4, &["PPC_NUMBE", "PPC_POS", "PPC_NAME", "PPC_FORM"], &[]),
    // Commanding.
    t("tcp", "TC packet headers", V4, &["TCP_ID", "TCP_DESC"], &[]),
    t("pcpc", "TC packet header parameters", V4, &["PCPC_PNAME", "PCPC_DESC", "PCPC_CODE"], &[]),
    t("pcdf", "TC packet header layout", V4, &["PCDF_TCNAME", "PCDF_DESC", "PCDF_TYPE", "PCDF_LEN", "PCDF_BIT", "PCDF_PNAME", "PCDF_VALUE", "PCDF_RADIX"], &[]),
    t("ccf", "telecommands", V4, &["CCF_CNAME", "CCF_DESCR", "CCF_DESCR2", "CCF_CTYPE", "CCF_CRITICAL", "CCF_PKTID", "CCF_TYPE", "CCF_STYPE", "CCF_APID", "CCF_NPARS", "CCF_PLAN", "CCF_EXEC", "CCF_ILSCOPE", "CCF_ILSTAGE", "CCF_SUBSYS", "CCF_HIPRI", "CCF_MAPID", "CCF_DEFSET", "CCF_RAPID", "CCF_ACK", "CCF_SUBSCHEDID"], &[("CCF_DEFSET", V5), ("CCF_ACK", V6), ("CCF_SUBSCHEDID", V6_9)]),
    t("dst", "telecommand destinations", V5, &["DST_APID", "DST_ROUTE"], &[]),
    t("cpc", "TC parameter characteristics", V4, &["CPC_NAME", "CPC_DESCR", "CPC_PTC", "CPC_PFC", "CPC_DISPFMT", "CPC_RADIX", "CPC_UNIT", "CPC_CATEG", "CPC_PRFREF", "CPC_CCAREF", "CPC_PAFREF", "CPC_INTER", "CPC_DEFVAL", "CPC_CORR", "CPC_OBTID", "CPC_DESCR2", "CPC_ENDIAN"], &[("CPC_CORR", V5), ("CPC_DESCR2", V6_9), ("CPC_ENDIAN", V7)]),
    t("cdf", "telecommand layouts", V4, &["CDF_CNAME", "CDF_ELTYPE", "CDF_DESCR", "CDF_ELLEN", "CDF_BIT", "CDF_GRPSIZE", "CDF_PNAME", "CDF_INTER", "CDF_VALUE", "CDF_TMID"], &[("CDF_TMID", V6)]),
    t("ptv", "telecommand pre-transmission validation", V5, &["PTV_CNAME", "PTV_PARNAM", "PTV_INTER", "PTV_VAL"], &[]),
    t("cvs", "telecommand verification stages", V4, &["CVS_ID", "CVS_TYPE", "CVS_SOURCE", "CVS_START", "CVS_INTERVAL", "CVS_SPID", "CVS_UNCERTAINTY"], &[]),
    t("cve", "telecommand verification expressions", V4, &["CVE_CVSID", "CVE_PARNAM", "CVE_INTER", "CVE_VAL", "CVE_TOL", "CVE_CHECK"], &[]),
    t("cvp", "telecommand verification profiles", V4, &["CVP_TASK", "CVP_TYPE", "CVP_CVSID"], &[]),
    t("pst", "TC parameter sets", V6, &["PST_NAME", "PST_DESCR"], &[]),
    t("psv", "TC parameter value sets", V6, &["PSV_NAME", "PSV_PVSID", "PSV_DESCR"], &[]),
    t("cps", "parameters in TC parameter sets", V6, &["CPS_NAME", "CPS_PAR", "CPS_BIT"], &[]),
    t("pvs", "values in TC parameter value sets", V6, &["PVS_ID", "PVS_PSID", "PVS_PNAME", "PVS_INTER", "PVS_VALS", "PVS_BIT"], &[]),
    t("psm", "TC parameter set mappings", V6, &["PSM_NAME", "PSM_TYPE", "PSM_PARSET"], &[]),
    t("cca", "TC numeric calibrations", V4, &["CCA_NUMBR", "CCA_DESCR", "CCA_ENGFMT", "CCA_RAWFMT", "CCA_RADIX", "CCA_UNIT", "CCA_NCURVE"], &[]),
    t("ccs", "TC numeric calibration points", V4, &["CCS_NUMBR", "CCS_XVALS", "CCS_YVALS"], &[]),
    t("paf", "TC text calibrations", V4, &["PAF_NUMBR", "PAF_DESCR", "PAF_RAWFMT", "PAF_NALIAS"], &[]),
    t("pas", "TC text calibration values", V4, &["PAS_NUMBR", "PAS_ALTXT", "PAS_ALVAL"], &[]),
    t("prf", "TC parameter range sets", V4, &["PRF_NUMBR", "PRF_DESCR", "PRF_INTER", "PRF_DSPFMT", "PRF_RADIX", "PRF_NRANGE", "PRF_UNIT"], &[]),
    t("prv", "TC parameter ranges", V4, &["PRV_NUMBR", "PRV_MINVAL", "PRV_MAXVAL"], &[]),
    // Sequences.
    t("csf", "command sequences", V4, &["CSF_NAME", "CSF_DESC", "CSF_DESC2", "CSF_IFTT", "CSF_NFPARS", "CSF_ELEMS", "CSF_CRITICAL", "CSF_PLAN", "CSF_EXEC", "CSF_SUBSYS", "CSF_GENTIME", "CSF_DOCNAME", "CSF_ISSUE", "CSF_DATE", "CSF_DEFSET", "CSF_SUBSCHEDID"], &[("CSF_DEFSET", V5), ("CSF_SUBSCHEDID", V6_9)]),
    t("css", "command sequence steps", V4, &["CSS_SQNAME", "CSS_COMM", "CSS_ENTRY", "CSS_TYPE", "CSS_ELEMID", "CSS_NPARS", "CSS_MANDISP", "CSS_RELTYPE", "CSS_RELTIME", "CSS_EXTIME", "CSS_PREVREL", "CSS_GROUP", "CSS_BLOCK", "CSS_ILSCOPE", "CSS_ILSTAGE", "CSS_DYNPTV", "CSS_STATPTV", "CSS_CEV"], &[("CSS_DYNPTV", V5)]),
    t("sdf", "command sequence step parameters", V4, &["SDF_SQNAME", "SDF_ENTRY", "SDF_ELEMID", "SDF_POS", "SDF_PNAME", "SDF_FTYPE", "SDF_VTYPE", "SDF_VALUE", "SDF_VALSET", "SDF_REPPOS"], &[("SDF_VALSET", V6)]),
    t("csp", "command sequence formal parameters", V4, &["CSP_SQNAME", "CSP_FPNAME", "CSP_FPNUM", "CSP_DESCR", "CSP_PTC", "CSP_PFC", "CSP_DISPFMT", "CSP_RADIX", "CSP_TYPE", "CSP_VTYPE", "CSP_DEFVAL", "CSP_CATEG", "CSP_PRFREF", "CSP_CCAREF", "CSP_PAFREF", "CSP_UNIT"], &[]),
];

fn table(name: &str) -> Option<&'static Table> {
    TABLES.iter().find(|t| t.name == name)
}

/// `name`'s columns in issue `icd`, or `None` when that issue doesn't
/// have the table.
fn layout(table: &Table, icd: Icd) -> Option<Vec<&'static str>> {
    if table.since > icd {
        return None;
    }
    let mut at = table.since;
    let mut out = Vec::with_capacity(table.columns.len());
    for column in table.columns {
        if let Some((_, since)) = table.added.iter().find(|(c, _)| c == column) {
            at = *since;
        }
        if at <= icd {
            out.push(*column);
        }
    }
    Some(out)
}

/// The column names for `table` (lower-case, e.g. `"ccf"`) in the
/// newest issue, or `None` if it's not one of the ICD's tables.
pub fn columns(table: &str) -> Option<&'static [&'static str]> {
    self::table(table).map(|t| t.columns)
}

/// What `table` holds, for a person: `"telecommands"`, `"TM packet
/// identification"`.
pub fn title(table: &str) -> Option<&'static str> {
    self::table(table).map(|t| t.title)
}

/// Every table any issue has, for `MibIndex::refresh` to scan for on
/// disk.
pub fn all_tables() -> impl Iterator<Item = &'static str> {
    TABLES.iter().map(|t| t.name)
}

/// The tables issue `icd` has.
pub fn tables_in(icd: Icd) -> impl Iterator<Item = &'static str> {
    TABLES.iter().filter(move |t| t.since <= icd).map(|t| t.name)
}

/// The issue a table first appeared in.
pub fn since(table: &str) -> Option<Icd> {
    self::table(table).map(|t| t.since)
}

/// How a project's MIBs are laid out: the issue they follow -- one for
/// all, worked out from each MIB's files, or one per MIB -- and the
/// tables whose columns the mission has written differently, or added.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Schema {
    /// The issue every MIB follows unless `per_root` names it; `None`:
    /// each MIB's is worked out from its files (`detect`).
    pub icd: Option<Icd>,
    /// A MIB's own issue, by its label; `None` here too means worked out.
    pub per_root: Vec<(String, Option<Icd>)>,
    /// A table's columns in file order, by table name, in place of the
    /// ICD's -- for any issue -- or a table of the mission's own.
    pub columns: Vec<(String, Vec<String>)>,
}

impl Default for Schema {
    fn default() -> Self {
        Schema { icd: Some(Icd::LATEST), per_root: Vec::new(), columns: Vec::new() }
    }
}

impl Schema {
    /// A layout for every MIB worked out from its files.
    pub fn auto() -> Schema {
        Schema { icd: None, ..Schema::default() }
    }

    /// The issue the MIB labelled `label` follows, `None` for "work it
    /// out".
    pub fn icd_for(&self, label: &str) -> Option<Icd> {
        self.per_root.iter().find(|(l, _)| l == label).map(|(_, icd)| *icd).unwrap_or(self.icd)
    }

    /// The columns of `table` in issue `icd`: the mission's own when it
    /// gave some, else the ICD's. `None`: neither knows the table, or the
    /// issue predates it.
    pub fn columns(&self, icd: Icd, table: &str) -> Option<Vec<String>> {
        if let Some((_, own)) = self.columns.iter().find(|(t, _)| t.eq_ignore_ascii_case(table)) {
            return Some(own.clone());
        }
        layout(self::table(table)?, icd).map(|c| c.into_iter().map(str::to_string).collect())
    }

    /// The table's columns whatever the issue: for a table the issue
    /// doesn't have that's on disk all the same.
    pub fn columns_any(&self, table: &str) -> Option<Vec<String>> {
        self.columns(Icd::LATEST, table)
    }

    /// Every table name worth looking for on disk: the ICD's, whatever
    /// the issue, then the mission's own.
    pub fn tables(&self) -> Vec<String> {
        let mut out: Vec<String> = all_tables().map(str::to_string).collect();
        for (t, _) in &self.columns {
            let t = t.to_ascii_lowercase();
            if !out.contains(&t) {
                out.push(t);
            }
        }
        out
    }

    /// Whether `table` is one the layout knows: the ICD's, or the
    /// mission's.
    pub fn knows(&self, table: &str) -> bool {
        self::table(table).is_some() || self.columns.iter().any(|(t, _)| t.eq_ignore_ascii_case(table))
    }

    /// The issue a MIB's files were written for, from their widths: each
    /// table's widest line is set against each issue's column count, and
    /// the issue that fits the most tables exactly -- with no line wider
    /// than it allows -- wins; the newest, when several do equally well
    /// or nothing tells them apart. `files` are (table, its text).
    pub fn detect<'a>(&self, files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Icd {
        let widths: Vec<(&str, usize)> = files
            .into_iter()
            .filter(|(table, _)| !self.columns.iter().any(|(t, _)| t.eq_ignore_ascii_case(table)))
            .map(|(table, text)| (table, text.lines().filter(|l| !l.is_empty()).map(|l| l.split('\t').count()).max().unwrap_or(0)))
            .filter(|(_, w)| *w > 0)
            .collect();
        let mut best = (i64::MIN, Icd::LATEST);
        for icd in Icd::ALL {
            let mut score = 0i64;
            for (table, width) in &widths {
                match self::table(table).and_then(|t| layout(t, icd)) {
                    Some(c) if c.len() == *width => score += 1,
                    Some(c) if c.len() < *width => score -= 100,
                    Some(_) => {}
                    // A table this issue doesn't have yet.
                    None if self::table(table).is_some() => score -= 10,
                    None => {}
                }
            }
            if score >= best.0 {
                best = (score, icd);
            }
        }
        best.1
    }

    /// `detect` on a MIB folder's files.
    pub fn detect_dir(&self, dir: &Path) -> Icd {
        let texts: Vec<(String, String)> = all_tables().filter_map(|t| Some((t.to_string(), std::fs::read_to_string(dir.join(format!("{t}.dat"))).ok()?))).collect();
        self.detect(texts.iter().map(|(t, s)| (t.as_str(), s.as_str())))
    }
}

/// A list of column names as the settings write them: separated by
/// commas, spaces or tabs.
pub fn parse_columns(text: &str) -> Vec<String> {
    text.split([',', ' ', '\t', ';']).map(str::trim).filter(|c| !c.is_empty()).map(str::to_ascii_uppercase).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_resolves_a_known_table() {
        assert_eq!(columns("prv"), Some(&["PRV_NUMBR", "PRV_MINVAL", "PRV_MAXVAL"][..]));
    }

    #[test]
    fn columns_is_none_for_an_unknown_table() {
        assert_eq!(columns("nope"), None);
    }

    #[test]
    fn all_tables_includes_every_schema_and_only_those() {
        let names: Vec<&str> = all_tables().collect();
        assert_eq!(names.len(), TABLES.len());
        for t in ["ccf", "cdf", "cpc", "pcf", "vpd", "cvs", "cve", "cvp", "grp", "dpc", "gpc", "spc", "ppc", "pvs", "cps", "psm"] {
            assert!(names.contains(&t), "{t}");
        }
    }

    #[test]
    fn no_duplicate_table_names() {
        let mut names: Vec<&str> = all_tables().collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before);
    }

    #[test]
    fn every_column_marker_names_a_column_of_its_table_in_order() {
        for t in TABLES {
            let mut last = t.since;
            for (c, since) in t.added {
                assert!(t.columns.contains(c), "{}: {c}", t.name);
                assert!(*since > t.since, "{}: {c}", t.name);
                assert!(*since >= last, "{}: {c} goes back an issue", t.name);
                last = *since;
            }
            // Column names carry their table's prefix.
            let prefix = format!("{}_", t.name.to_ascii_uppercase());
            assert!(t.columns.iter().all(|c| c.starts_with(&prefix)), "{}", t.name);
        }
    }

    #[test]
    fn each_issue_is_a_prefix_of_the_next() {
        for t in TABLES {
            let mut previous: Option<Vec<&str>> = None;
            for icd in Icd::ALL {
                let Some(now) = layout(t, icd) else { continue };
                if let Some(p) = &previous {
                    assert!(now.starts_with(p), "{} {icd}", t.name);
                }
                previous = Some(now);
            }
            assert_eq!(previous.as_deref(), Some(t.columns), "{}: the newest issue has every column", t.name);
        }
    }

    #[test]
    fn older_issues_have_fewer_columns_and_tables() {
        let s = Schema::default();
        assert_eq!(s.columns(Icd::V7, "ccf").unwrap().len(), 21);
        assert_eq!(s.columns(Icd::V6_9, "ccf").unwrap().len(), 21);
        assert_eq!(s.columns(Icd::V6, "ccf").unwrap().len(), 20);
        assert_eq!(s.columns(Icd::V5, "ccf").unwrap().len(), 19);
        assert_eq!(s.columns(Icd::V4, "ccf").unwrap().len(), 17);
        assert_eq!(s.columns(Icd::V4, "pst"), None);
        assert!(tables_in(Icd::V4).count() < tables_in(Icd::V7).count());
    }

    #[test]
    fn issues_parse_the_way_people_write_them() {
        assert_eq!(Icd::parse("7.2"), Some(Icd::V7));
        assert_eq!(Icd::parse("ICD 7"), Some(Icd::V7));
        assert_eq!(Icd::parse("6.9"), Some(Icd::V6_9));
        assert_eq!(Icd::parse("6.3"), Some(Icd::V6));
        assert_eq!(Icd::parse("6.x"), Some(Icd::V6));
        assert_eq!(Icd::parse("5.x"), Some(Icd::V5));
        assert_eq!(Icd::parse("4"), Some(Icd::V4));
        assert_eq!(Icd::parse("3.1"), None);
        assert_eq!(Icd::parse("auto"), None);
        for icd in Icd::ALL {
            assert_eq!(Icd::parse(icd.label()), Some(icd));
        }
    }

    #[test]
    fn a_missions_own_columns_take_the_place_of_the_icds() {
        let s = Schema { columns: vec![("PRV".into(), vec!["PRV_NUMBR".into(), "PRV_MINVAL".into(), "PRV_MAXVAL".into(), "PRV_X".into()]), ("zzz".into(), vec!["ZZZ_A".into()])], ..Schema::default() };
        assert_eq!(s.columns(Icd::V4, "prv").unwrap().len(), 4);
        assert_eq!(s.columns(Icd::V7, "zzz"), Some(vec!["ZZZ_A".to_string()]));
        assert!(s.tables().contains(&"zzz".to_string()));
        assert!(s.knows("zzz") && s.knows("ccf") && !s.knows("yyy"));
    }

    #[test]
    fn a_per_mib_issue_wins_over_the_projects() {
        let s = Schema { icd: Some(Icd::V7), per_root: vec![("OLD".into(), Some(Icd::V5)), ("ANY".into(), None)], columns: Vec::new() };
        assert_eq!(s.icd_for("OLD"), Some(Icd::V5));
        assert_eq!(s.icd_for("ANY"), None);
        assert_eq!(s.icd_for("NEW"), Some(Icd::V7));
    }

    fn line(n: usize) -> String {
        vec!["x"; n].join("\t") + "\n"
    }

    #[test]
    fn detect_picks_the_issue_whose_widths_fit() {
        let s = Schema::auto();
        let ccf = line(19);
        let pcf = line(21);
        let pid = line(14);
        assert_eq!(s.detect([("ccf", ccf.as_str()), ("pcf", pcf.as_str()), ("pid", pid.as_str())]), Icd::V5);
        let ccf = line(21);
        let pcf = line(24);
        assert_eq!(s.detect([("ccf", ccf.as_str()), ("pcf", pcf.as_str())]), Icd::V7);
        // Nothing to tell them apart: the newest.
        let prv = line(3);
        assert_eq!(s.detect([("prv", prv.as_str())]), Icd::V7);
        assert_eq!(s.detect([]), Icd::V7);
    }

    #[test]
    fn detect_never_picks_an_issue_a_line_is_too_wide_for() {
        let s = Schema::auto();
        let ccf = line(17);
        let pcf = line(24); // only 7.x has 24 PCF columns
        let pid = line(14);
        let tpcf = line(2);
        assert_eq!(s.detect([("ccf", ccf.as_str()), ("pid", pid.as_str()), ("tpcf", tpcf.as_str()), ("pcf", pcf.as_str())]), Icd::V7);
    }

    #[test]
    fn column_lists_parse_from_settings_text() {
        assert_eq!(parse_columns("a, b c;d"), vec!["A", "B", "C", "D"]);
    }
}
