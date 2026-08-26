//! SCOS-2000 MIB table column layouts, per the Database Import ICD --
//! only version 7.2 is supported, transcribed directly from the
//! reference implementation this crate ports
//! (orbit-emacs's `mod-mib--schemas-7.2`). No user-configurable custom
//! schemas: every table this crate's callers need (telecommand/TM
//! packet/TM parameter/calibration lookup and insertion) is one of these
//! 35 built-in ones.

/// `(table name, column names in file order)` -- table names are
/// lower-case, matching the `.dat` file's own base name (`ccf.dat`,
/// `pcf.dat`, ...).
const SCHEMAS_7_2: &[(&str, &[&str])] = &[
    ("caf", &["CAF_NUMBR", "CAF_DESCR", "CAF_ENGFMT", "CAF_RAWFMT", "CAF_RADIX", "CAF_UNIT", "CAF_NCURVE", "CAF_INTER"]),
    ("cap", &["CAP_NUMBR", "CAP_XVALS", "CAP_YVALS"]),
    ("cca", &["CCA_NUMBR", "CCA_DESCR", "CCA_ENGFMT", "CCA_RAWFMT", "CCA_RADIX", "CCA_UNIT", "CCA_NCURVE"]),
    (
        "ccf",
        &[
            "CCF_CNAME", "CCF_DESCR", "CCF_DESCR2", "CCF_CTYPE", "CCF_CRITICAL", "CCF_PKTID", "CCF_TYPE", "CCF_STYPE",
            "CCF_APID", "CCF_NPARS", "CCF_PLAN", "CCF_EXEC", "CCF_ILSCOPE", "CCF_ILSTAGE", "CCF_SUBSYS", "CCF_HIPRI",
            "CCF_MAPID", "CCF_DEFSET", "CCF_RAPID", "CCF_ACK", "CCF_SUBSCHEDID",
        ],
    ),
    ("ccs", &["CCS_NUMBR", "CCS_XVALS", "CCS_YVALS"]),
    (
        "cdf",
        &[
            "CDF_CNAME", "CDF_ELTYPE", "CDF_DESCR", "CDF_ELLEN", "CDF_BIT", "CDF_GRPSIZE", "CDF_PNAME", "CDF_INTER",
            "CDF_VALUE", "CDF_TMID",
        ],
    ),
    (
        "cpc",
        &[
            "CPC_NAME", "CPC_DESCR", "CPC_PTC", "CPC_PFC", "CPC_DISPFMT", "CPC_RADIX", "CPC_UNIT", "CPC_CATEG",
            "CPC_PRFREF", "CPC_CCAREF", "CPC_PAFREF", "CPC_INTER", "CPC_DEFVAL", "CPC_CORR", "CPC_OBTIP", "CPC_DESCR2",
            "CPC_ENDIAN",
        ],
    ),
    (
        "csf",
        &[
            "CSF_NAME", "CSF_DESC", "CSF_DESC2", "CSF_IFTT", "CSF_NFPARS", "CSF_ELEMS", "CSF_CRITICAL", "CSF_PLAN",
            "CSF_EXEC", "CSF_SUBSYS", "CSF_GENTIME", "CSF_DOCNAME", "CSF_ISSUE", "CSF_DATE", "CSF_DEFSET",
            "CSF_SUBSCHEDID",
        ],
    ),
    (
        "csp",
        &[
            "CSP_SQNAME", "CSP_FPNAME", "CSP_FPNUM", "CSP_DESCR", "CSP_PTC", "CSP_PFC", "CSP_DISPFMT", "CSP_RADIX",
            "CSP_TYPE", "CSP_VTYPE", "CSP_DEFVAL", "CSP_CATEG", "CSP_PRFREF", "CSP_CCAREF", "CSP_PAFREF", "CSP_UNIT",
        ],
    ),
    (
        "css",
        &[
            "CSS_SQNAME", "CSS_COMM", "CSS_ENTRY", "CSS_TYPE", "CSS_ELEMID", "CSS_NPARS", "CSS_MANDISP", "CSS_RELTYPE",
            "CSS_RELTIME", "CSS_EXTIME", "CSS_PREVREL", "CSS_GROUP", "CSS_BLOCK", "CSS_ILSCOPE", "CSS_ILSTAGE",
            "CSS_DYNPTV", "CSS_STATPTV", "CSS_CEV",
        ],
    ),
    ("cur", &["CUR_PNAME", "CUR_POS", "CUR_RLCHK", "CUR_VALPAR", "CUR_SELECT"]),
    ("dst", &["DST_APID", "DST_ROUTE"]),
    ("lgf", &["LGF_IDENT", "LGF_DESCR", "LGF_POL1", "LGF_POL2", "LGF_POL3", "LGF_POL4", "LGF_POL5"]),
    ("mcf", &["MCF_IDENT", "MCF_DESCR", "MCF_POL1", "MCF_POL2", "MCF_POL3", "MCF_POL4", "MCF_POL5"]),
    ("ocf", &["OCF_NAME", "OCF_NBCHCK", "OCF_NBOOL", "OCF_INTER", "OCF_CODIN"]),
    ("ocp", &["OCP_NAME", "OCP_POS", "OCP_TYPE", "OCP_LVALU", "OCP_HVALU", "OCP_RLCHK", "OCP_VALPAR"]),
    ("paf", &["PAF_NUMBR", "PAF_DESCR", "PAF_RAWFMT", "PAF_NALIAS"]),
    ("pas", &["PAS_NUMBR", "PAS_ALTXT", "PAS_ALVAL"]),
    ("pcdf", &["PCDF_TCNAME", "PCDF_DESC", "PCDF_TYPE", "PCDF_LEN", "PCDF_BIT", "PCDF_PNAME", "PCDF_VALUE", "PCDF_RADIX"]),
    (
        "pcf",
        &[
            "PCF_NAME", "PCF_DESCR", "PCF_PID", "PCF_UNIT", "PCF_PTC", "PCF_PFC", "PCF_WIDTH", "PCF_VALID",
            "PCF_RELATED", "PCF_CATEG", "PCF_NATUR", "PCF_CURTX", "PCF_INTER", "PCF_USCON", "PCF_DECIM", "PCF_PARVAL",
            "PCF_SUBSYS", "PCF_VALPAR", "PCF_SPTYPE", "PCF_CORR", "PCF_OBTID", "PCF_DARC", "PCF_ENDIAN", "PCF_DESCR2",
        ],
    ),
    ("pcpc", &["PCPC_PNAME", "PCPC_DESC", "PCPC_CODE"]),
    ("pic", &["PIC_TYPE", "PIC_STYPE", "PIC_PI1_OFF", "PIC_PI1_WID", "PIC_PI2_OFF", "PIC_PI2_WID", "PIC_APID"]),
    (
        "pid",
        &[
            "PID_TYPE", "PID_STYPE", "PID_APID", "PID_PI1_VAL", "PID_PI2_VAL", "PID_SPID", "PID_DESCR", "PID_UNIT",
            "PID_TPSD", "PID_DFHSIZE", "PID_TIME", "PID_INTER", "PID_VALID", "PID_CHECK", "PID_EVENT", "PID_EVID",
        ],
    ),
    ("plf", &["PLF_NAME", "PLF_SPID", "PLF_OFFBY", "PLF_OFFBI", "PLF_NBOCC", "PLF_LGOCC", "PLF_TIME", "PLF_TDOCC"]),
    ("prf", &["PRF_NUMBR", "PRF_DESCR", "PRF_INTER", "PRF_DSPFMT", "PRF_RADIX", "PRF_NRANGE", "PRF_UNIT"]),
    ("prv", &["PRV_NUMBR", "PRV_MINVAL", "PRV_MAXVAL"]),
    ("pst", &["PST_NAME", "PST_DESCR"]),
    ("psv", &["PSV_NAME", "PSV_PVSID", "PSV_DESCR"]),
    ("ptv", &["PTV_CNAME", "PTV_PARNAM", "PTV_INTER", "PTV_VAL"]),
    (
        "sdf",
        &[
            "SDF_SQNAME", "SDF_ENTRY", "SDF_ELEMID", "SDF_POS", "SDF_PNAME", "SDF_FTYPE", "SDF_VTYPE", "SDF_VALUE",
            "SDF_VALSET", "SDF_REPPOS",
        ],
    ),
    ("tcp", &["TCP_ID", "TCP_DESC"]),
    ("tpcf", &["TPCF_SPID", "TPCF_NAME", "TPCF_SIZE"]),
    ("txf", &["TXF_NUMBR", "TXF_DESCR", "TXF_RAWFMT", "TXF_NALIAS"]),
    ("txp", &["TXP_NUMBR", "TXP_FROM", "TXP_TO", "TXP_ALTXT"]),
    ("vdf", &["VDF_NAME", "VDF_COMMENT"]),
    (
        "vpd",
        &[
            "VPD_TPSD", "VPD_POS", "VPD_NAME", "VPD_GRPSIZE", "VPD_FIXREP", "VPD_CHOICE", "VPD_PIDREF", "VPD_DISDESC",
            "VPD_WIDTH", "VPD_JUSTIFY", "VPD_NEWLINE", "VPD_DCHAR", "VPD_FORM", "VPD_OFFSET",
        ],
    ),
];

/// The column names for `table` (lower-case, e.g. `"ccf"`), or `None` if
/// it's not one of the 35 known ICD 7.2 tables.
pub fn columns(table: &str) -> Option<&'static [&'static str]> {
    SCHEMAS_7_2.iter().find(|(name, _)| *name == table).map(|(_, cols)| *cols)
}

/// Every known table name, for `MibIndex::refresh` to scan for on disk.
pub fn all_tables() -> impl Iterator<Item = &'static str> {
    SCHEMAS_7_2.iter().map(|(name, _)| *name)
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
        assert_eq!(names.len(), SCHEMAS_7_2.len());
        assert!(names.contains(&"ccf"));
        assert!(names.contains(&"cdf"));
        assert!(names.contains(&"cpc"));
        assert!(names.contains(&"pcf"));
        assert!(names.contains(&"vpd"));
    }

    #[test]
    fn no_duplicate_table_names() {
        let mut names: Vec<&str> = all_tables().collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before);
    }
}
