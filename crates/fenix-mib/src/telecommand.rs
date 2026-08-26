//! Telecommand-specific MIB queries: a TC's parameters (fixed vs.
//! variable), each variable parameter's engineering "domain" (unit,
//! default, allowed aliases/ranges), validating a typed value against
//! that domain, a parameter's calibration/reference rows, and rendering
//! the final command text from a user-configured template. Ported from
//! the reference elisp implementation's `mod-mib--tc-parameters` through
//! `mod-mib--render-telecommand`.

use crate::row::Row;
use crate::MibIndex;

/// One of a telecommand's parameters (a `CDF` row, joined with its `CPC`
/// definition if one exists by name).
#[derive(Debug, Clone)]
pub struct TcParameter {
    pub name: String,
    pub cdf: Row,
    pub cpc: Option<Row>,
    /// Whether `CDF_VALUE` is non-empty -- a fixed parameter has its
    /// value baked into the telecommand definition itself and is never
    /// prompted for or included in `{arguments}`.
    pub fixed: bool,
}

/// Engineering-value metadata for one variable parameter -- what a host
/// UI needs to prompt for and validate a value: a human description,
/// unit, default, PTC/PFC (SCOS-2000's own type/format codes), any
/// enumerated engineering aliases (from a `PAF`/`PAS` status
/// calibration), any numeric ranges (from a `PRF`/`PRV` range check),
/// and whether the parameter is numeric-typed at all.
#[derive(Debug, Clone, Default)]
pub struct ParamDomain {
    pub description: String,
    pub unit: String,
    pub default: String,
    pub ptc: String,
    pub pfc: String,
    pub aliases: Vec<String>,
    pub ranges: Vec<(f64, f64)>,
    pub numeric: bool,
}

pub const DEFAULT_TEMPLATE: &str =
    "telecommand_send PUS_T={type} PUS_ST={stype} APID={apid} MNEMO={mnemo} ARGUMENTS=[{arguments}]";
pub const DEFAULT_ARGUMENT_TEMPLATE: &str = "{name}={value}";
pub const DEFAULT_ARGUMENT_SEPARATOR: &str = ", ";

/// Every `CDF` row for `ccf` (a telecommand row from the `ccf` table),
/// each joined with its `CPC` definition (by `CDF_PNAME`/`CPC_NAME`) if
/// one exists -- scoped to `ccf`'s own MIB root (`ccf.source.root_index`),
/// so a same-named parameter defined in a *different* configured root
/// never gets pulled in by mistake.
pub fn tc_parameters(index: &MibIndex, ccf: &Row) -> Vec<TcParameter> {
    let root = Some(ccf.source.root_index);
    let name = ccf.clean("CCF_CNAME");
    index
        .rows_by_field("cdf", "CDF_CNAME", name, root)
        .into_iter()
        .map(|cdf| {
            let pname = cdf.clean("CDF_PNAME").to_string();
            let cpc = index.first_row_by_field("cpc", "CPC_NAME", &pname, root).cloned();
            let fixed = !cdf.clean("CDF_VALUE").is_empty();
            TcParameter { name: pname, cdf: cdf.clone(), cpc, fixed }
        })
        .collect()
}

/// `param`'s engineering-value metadata, resolved from its `CPC` (if
/// any) and, transitively, that `CPC`'s calibration/range references.
pub fn parameter_domain(index: &MibIndex, param: &TcParameter) -> ParamDomain {
    let description = {
        let cdf_descr = param.cdf.clean("CDF_DESCR");
        if !cdf_descr.is_empty() {
            cdf_descr.to_string()
        } else {
            param.cpc.as_ref().map(|c| c.clean("CPC_DESCR").to_string()).unwrap_or_default()
        }
    };
    ParamDomain {
        description,
        unit: param.cpc.as_ref().map(|c| c.clean("CPC_UNIT").to_string()).unwrap_or_default(),
        default: param.cpc.as_ref().map(|c| c.clean("CPC_DEFVAL").to_string()).unwrap_or_default(),
        ptc: param.cpc.as_ref().map(|c| c.clean("CPC_PTC").to_string()).unwrap_or_default(),
        pfc: param.cpc.as_ref().map(|c| c.clean("CPC_PFC").to_string()).unwrap_or_default(),
        aliases: param.cpc.as_ref().map(|c| alias_values(index, c)).unwrap_or_default(),
        ranges: param.cpc.as_ref().map(|c| range_pairs(index, c)).unwrap_or_default(),
        numeric: param.cpc.as_ref().is_some_and(numeric_cpc),
    }
}

fn alias_values(index: &MibIndex, cpc: &Row) -> Vec<String> {
    let root = Some(cpc.source.root_index);
    let reference = cpc.clean("CPC_PAFREF");
    if reference.is_empty() {
        return Vec::new();
    }
    index
        .rows_by_field("pas", "PAS_NUMBR", reference, root)
        .into_iter()
        .map(|row| row.clean("PAS_ALTXT").to_string())
        .filter(|alias| !alias.is_empty())
        .collect()
}

fn range_pairs(index: &MibIndex, cpc: &Row) -> Vec<(f64, f64)> {
    let root = Some(cpc.source.root_index);
    let reference = cpc.clean("CPC_PRFREF");
    if reference.is_empty() {
        return Vec::new();
    }
    index
        .rows_by_field("prv", "PRV_NUMBR", reference, root)
        .into_iter()
        .filter_map(|row| {
            let min = row.clean("PRV_MINVAL");
            let max = row.clean("PRV_MAXVAL");
            if number_string(min) && number_string(max) {
                Some((min.parse().ok()?, max.parse().ok()?))
            } else {
                None
            }
        })
        .collect()
}

fn numeric_cpc(cpc: &Row) -> bool {
    matches!(cpc.clean("CPC_PTC"), "1" | "2" | "3" | "4")
}

/// Direct calibration/reference rows for `cpc` -- `CCA`/`CAF`/`CCS` via
/// `CPC_CCAREF`, `PAF`/`PAS` via `CPC_PAFREF`, `PRF`/`PRV` via
/// `CPC_PRFREF` -- scoped to `cpc`'s own MIB root, same reasoning as
/// `tc_parameters`.
pub fn calibration_rows(index: &MibIndex, cpc: &Row) -> Vec<Row> {
    let root = Some(cpc.source.root_index);
    let mut rows = Vec::new();
    let cca_ref = cpc.clean("CPC_CCAREF");
    if !cca_ref.is_empty() {
        for table in ["cca", "caf", "ccs"] {
            rows.extend(index.rows_by_field(table, &format!("{}_NUMBR", table.to_uppercase()), cca_ref, root).into_iter().cloned());
        }
    }
    let paf_ref = cpc.clean("CPC_PAFREF");
    if !paf_ref.is_empty() {
        rows.extend(index.rows_by_field("paf", "PAF_NUMBR", paf_ref, root).into_iter().cloned());
        rows.extend(index.rows_by_field("pas", "PAS_NUMBR", paf_ref, root).into_iter().cloned());
    }
    let prf_ref = cpc.clean("CPC_PRFREF");
    if !prf_ref.is_empty() {
        rows.extend(index.rows_by_field("prf", "PRF_NUMBR", prf_ref, root).into_iter().cloned());
        rows.extend(index.rows_by_field("prv", "PRV_NUMBR", prf_ref, root).into_iter().cloned());
    }
    rows
}

/// Whether `value` (trimmed) looks like a plain decimal number: an
/// optional leading `+`/`-`, digits, and an optional `.digits` -- no
/// exponents, no `inf`/`nan`, deliberately narrower than `str::parse`
/// would accept, matching the reference elisp implementation's own
/// `[+-]?[0-9]+(?:\.[0-9]+)?` pattern.
fn number_string(value: &str) -> bool {
    let v = value.trim();
    let v = v.strip_prefix(['+', '-']).unwrap_or(v);
    let (int_part, frac_part) = match v.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (v, None),
    };
    if int_part.is_empty() || !int_part.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    match frac_part {
        Some(f) => !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()),
        None => true,
    }
}

fn format_ranges(ranges: &[(f64, f64)]) -> String {
    ranges.iter().map(|(lo, hi)| format!("{lo}..{hi}")).collect::<Vec<_>>().join(", ")
}

/// Warning strings for `value` as an engineering value for `param`, per
/// `domain` -- empty when there's nothing to warn about. Never rejects
/// the value outright (the host still lets the user insert past a
/// warning, same as the reference implementation's own "insert anyway?"
/// confirmation) -- this only ever informs.
pub fn validate_argument(param: &TcParameter, value: &str, domain: &ParamDomain) -> Vec<String> {
    let value = value.trim();
    let mut warnings = Vec::new();
    let has_aliases = !domain.aliases.is_empty();
    let known_alias = has_aliases && domain.aliases.iter().any(|alias| alias == value);
    if has_aliases && !known_alias {
        warnings.push(format!(
            "{}: '{value}' is not one of the known engineering aliases: {}",
            param.name,
            domain.aliases.join(", ")
        ));
    }
    if !domain.ranges.is_empty() && !has_aliases {
        if !number_string(value) {
            warnings.push(format!("{}: '{value}' is not numeric for range validation", param.name));
        } else {
            let number: f64 = value.parse().unwrap_or(f64::NAN);
            let in_range = domain.ranges.iter().any(|(lo, hi)| *lo <= number && number <= *hi);
            if !in_range {
                warnings.push(format!(
                    "{}: '{value}' is outside the known engineering range(s): {}",
                    param.name,
                    format_ranges(&domain.ranges)
                ));
            }
        }
    }
    if domain.ranges.is_empty() && !has_aliases && domain.numeric && !number_string(value) {
        warnings.push(format!("{}: '{value}' is not numeric for PTC/PFC {}/{}", param.name, domain.ptc, domain.pfc));
    }
    warnings
}

fn replace_placeholders(template: &str, values: &[(&str, &str)]) -> String {
    let mut text = template.to_string();
    for (key, value) in values {
        text = text.replace(&format!("{{{key}}}"), value);
    }
    text
}

/// Renders `ccf` and its collected `arguments` (`(name, value)` pairs,
/// already excluding fixed parameters) using `template`/`arg_template`/
/// `arg_separator` -- `{type}`/`{stype}`/`{apid}`/`{mnemo}`/
/// `{description}`/`{mib}`/`{arguments}` in `template`, `{name}`/
/// `{value}` in `arg_template` for each argument, joined by
/// `arg_separator`.
pub fn render_telecommand(
    template: &str,
    arg_template: &str,
    arg_separator: &str,
    ccf: &Row,
    root_label: &str,
    arguments: &[(String, String)],
) -> String {
    let argument_text = arguments
        .iter()
        .map(|(name, value)| replace_placeholders(arg_template, &[("name", name), ("value", value)]))
        .collect::<Vec<_>>()
        .join(arg_separator);
    replace_placeholders(
        template,
        &[
            ("type", ccf.clean("CCF_TYPE")),
            ("stype", ccf.clean("CCF_STYPE")),
            ("apid", ccf.clean("CCF_APID")),
            ("mnemo", ccf.clean("CCF_CNAME")),
            ("description", ccf.clean("CCF_DESCR")),
            ("mib", root_label),
            ("arguments", &argument_text),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MibRoot;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_root(name: &str) -> MibRoot {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("fenix-mib-tc-test-{name}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        MibRoot { label: name.to_uppercase(), path: dir }
    }

    fn write(root: &MibRoot, table: &str, contents: &str) {
        std::fs::write(root.path.join(format!("{table}.dat")), contents).unwrap();
    }

    /// A full, realistic little MIB: one TC (`AAA001`) with a fixed
    /// parameter, an aliased (`PAF`/`PAS`) parameter, and a ranged
    /// (`PRF`/`PRV`) parameter.
    fn build_fixture(root: &MibRoot) {
        write(root, "ccf", "AAA001\tSwitch mode\t\tA\t0\t1\t8\t1\t100\t3\t\t\t\t\tAOCS\t\t\t\t\t\t\n");
        write(
            root,
            "cdf",
            "AAA001\tA\tsubtype\t8\t0\t0\tSTYPE\t\t1\t\nAAA001\tA\tmode\t8\t8\t0\tMODE\t\t\t\nAAA001\tA\tgain\t16\t16\t0\tGAIN\t\t\t\n",
        );
        write(
            root,
            "cpc",
            "MODE\tOperating mode\t7\t1\tA\t\t\tS\t\t\tPAF1\t\t\t\tOFF\t\tno\nGAIN\tLoop gain\t3\t1\tA\t\t\tS\tPRF1\t\t\t\t\t\t5\t\tno\n",
        );
        write(root, "paf", "PAF1\tMode select\tU\t2\n");
        write(root, "pas", "PAF1\tOFF\t0\nPAF1\tON\t1\n");
        write(root, "prf", "PRF1\tGain range\tno\t\t\t1\t\n");
        write(root, "prv", "PRF1\t0\t100\n");
    }

    #[test]
    fn tc_parameters_splits_fixed_from_variable_and_joins_cpc() {
        let root = temp_root("params");
        build_fixture(&root);
        let mut index = MibIndex::new(vec![root.clone()]);
        index.refresh();
        let ccf = index.first_row_by_field("ccf", "CCF_CNAME", "AAA001", None).unwrap();

        let params = tc_parameters(&index, ccf);

        assert_eq!(params.len(), 3);
        let subtype = params.iter().find(|p| p.name == "STYPE").unwrap();
        assert!(subtype.fixed); // CDF_VALUE = "1"
        assert!(subtype.cpc.is_none()); // no CPC row named "STYPE" in the fixture

        let mode = params.iter().find(|p| p.name == "MODE").unwrap();
        assert!(!mode.fixed);
        assert_eq!(mode.cpc.as_ref().unwrap().get("CPC_NAME"), Some("MODE"));

        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn parameter_domain_resolves_aliases_from_paf_pas() {
        let root = temp_root("aliases");
        build_fixture(&root);
        let mut index = MibIndex::new(vec![root.clone()]);
        index.refresh();
        let ccf = index.first_row_by_field("ccf", "CCF_CNAME", "AAA001", None).unwrap();
        let mode = tc_parameters(&index, ccf).into_iter().find(|p| p.name == "MODE").unwrap();

        let domain = parameter_domain(&index, &mode);

        assert_eq!(domain.aliases, vec!["OFF".to_string(), "ON".to_string()]);
        assert!(domain.ranges.is_empty());
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn parameter_domain_resolves_ranges_from_prf_prv() {
        let root = temp_root("ranges");
        build_fixture(&root);
        let mut index = MibIndex::new(vec![root.clone()]);
        index.refresh();
        let ccf = index.first_row_by_field("ccf", "CCF_CNAME", "AAA001", None).unwrap();
        let gain = tc_parameters(&index, ccf).into_iter().find(|p| p.name == "GAIN").unwrap();

        let domain = parameter_domain(&index, &gain);

        assert!(domain.aliases.is_empty());
        assert_eq!(domain.ranges, vec![(0.0, 100.0)]);
        assert!(domain.numeric); // CPC_PTC "3"
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn calibration_rows_collects_paf_and_pas_for_an_aliased_parameter() {
        let root = temp_root("cal-rows");
        build_fixture(&root);
        let mut index = MibIndex::new(vec![root.clone()]);
        index.refresh();
        let ccf = index.first_row_by_field("ccf", "CCF_CNAME", "AAA001", None).unwrap();
        let mode = tc_parameters(&index, ccf).into_iter().find(|p| p.name == "MODE").unwrap();

        let rows = calibration_rows(&index, mode.cpc.as_ref().unwrap());

        assert_eq!(rows.iter().filter(|r| r.table == "paf").count(), 1);
        assert_eq!(rows.iter().filter(|r| r.table == "pas").count(), 2);
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn validate_argument_flags_an_unknown_alias() {
        let param = TcParameter {
            name: "MODE".to_string(),
            cdf: dummy_row("cdf"),
            cpc: Some(dummy_row("cpc")),
            fixed: false,
        };
        let domain = ParamDomain { aliases: vec!["OFF".to_string(), "ON".to_string()], ..Default::default() };
        let warnings = validate_argument(&param, "STANDBY", &domain);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("STANDBY"));
    }

    #[test]
    fn validate_argument_accepts_a_known_alias() {
        let param = TcParameter { name: "MODE".to_string(), cdf: dummy_row("cdf"), cpc: None, fixed: false };
        let domain = ParamDomain { aliases: vec!["OFF".to_string(), "ON".to_string()], ..Default::default() };
        assert!(validate_argument(&param, "ON", &domain).is_empty());
    }

    #[test]
    fn validate_argument_flags_a_value_outside_the_known_range() {
        let param = TcParameter { name: "GAIN".to_string(), cdf: dummy_row("cdf"), cpc: None, fixed: false };
        let domain = ParamDomain { ranges: vec![(0.0, 100.0)], numeric: true, ..Default::default() };
        let warnings = validate_argument(&param, "500", &domain);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("outside"));
    }

    #[test]
    fn validate_argument_accepts_a_value_inside_the_known_range() {
        let param = TcParameter { name: "GAIN".to_string(), cdf: dummy_row("cdf"), cpc: None, fixed: false };
        let domain = ParamDomain { ranges: vec![(0.0, 100.0)], numeric: true, ..Default::default() };
        assert!(validate_argument(&param, "50", &domain).is_empty());
    }

    #[test]
    fn validate_argument_flags_a_non_numeric_value_for_a_numeric_parameter_with_no_range_or_aliases() {
        let param = TcParameter { name: "GAIN".to_string(), cdf: dummy_row("cdf"), cpc: None, fixed: false };
        let domain = ParamDomain { numeric: true, ptc: "3".to_string(), pfc: "1".to_string(), ..Default::default() };
        let warnings = validate_argument(&param, "not-a-number", &domain);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("PTC/PFC 3/1"));
    }

    #[test]
    fn validate_argument_is_silent_for_a_non_numeric_parameter_with_no_constraints() {
        let param = TcParameter { name: "LABEL".to_string(), cdf: dummy_row("cdf"), cpc: None, fixed: false };
        let domain = ParamDomain::default();
        assert!(validate_argument(&param, "anything at all", &domain).is_empty());
    }

    #[test]
    fn render_telecommand_substitutes_every_placeholder() {
        let root = temp_root("render");
        build_fixture(&root);
        let mut index = MibIndex::new(vec![root.clone()]);
        index.refresh();
        let ccf = index.first_row_by_field("ccf", "CCF_CNAME", "AAA001", None).unwrap();

        let arguments = vec![("MODE".to_string(), "ON".to_string()), ("GAIN".to_string(), "50".to_string())];
        let command = render_telecommand(
            DEFAULT_TEMPLATE,
            DEFAULT_ARGUMENT_TEMPLATE,
            DEFAULT_ARGUMENT_SEPARATOR,
            ccf,
            &root.label,
            &arguments,
        );

        assert_eq!(
            command,
            "telecommand_send PUS_T=8 PUS_ST=1 APID=100 MNEMO=AAA001 ARGUMENTS=[MODE=ON, GAIN=50]"
        );
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn render_telecommand_with_no_arguments_leaves_the_placeholder_empty() {
        let root = temp_root("render-empty");
        build_fixture(&root);
        let mut index = MibIndex::new(vec![root.clone()]);
        index.refresh();
        let ccf = index.first_row_by_field("ccf", "CCF_CNAME", "AAA001", None).unwrap();

        let command =
            render_telecommand(DEFAULT_TEMPLATE, DEFAULT_ARGUMENT_TEMPLATE, DEFAULT_ARGUMENT_SEPARATOR, ccf, &root.label, &[]);

        assert!(command.ends_with("ARGUMENTS=[]"));
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn render_telecommand_honors_a_custom_template() {
        let root = temp_root("render-custom");
        build_fixture(&root);
        let mut index = MibIndex::new(vec![root.clone()]);
        index.refresh();
        let ccf = index.first_row_by_field("ccf", "CCF_CNAME", "AAA001", None).unwrap();

        let command = render_telecommand("{mnemo} from {mib}: {arguments}", "{name}:{value}", "|", ccf, &root.label, &[("MODE".to_string(), "ON".to_string())]);

        assert_eq!(command, format!("AAA001 from {}: MODE:ON", root.label));
        std::fs::remove_dir_all(&root.path).ok();
    }

    fn dummy_row(table: &str) -> Row {
        Row {
            table: table.to_string(),
            fields: Vec::new(),
            source: crate::RowSource {
                root_index: 0,
                root_label: "TEST".to_string(),
                table: table.to_string(),
                file: std::path::PathBuf::from(format!("{table}.dat")),
                line: 1,
            },
        }
    }
}
