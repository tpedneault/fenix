//! Doxygen's XML output as a source of documentation for user-defined
//! Tcl procs -- the brief, the `@param` table and the `@return` note a
//! proc was documented with, keyed by the same qualified name the ctags
//! index uses (`util::greet`), so hover and completion can show docs
//! next to the signature ctags already provides.
//!
//! Optional: a project opts in with a `[doxygen]` section in `.fenix/
//! project.ini` naming the directory `GENERATE_XML = YES` wrote to
//! (`xml_dir = docs/xml`, relative to the project root). Generating the
//! XML stays the build's job -- this only reads what the last `doxygen`
//! run left behind, which also keeps the pinned version (1.8.17 was the
//! last with a Tcl parser) out of the editor's concern.
//!
//! Written against the `compound.xsd` shape Doxygen 1.8.x emits:
//! `index.xml` lists every compound with its `refid`; each `<refid>.xml`
//! holds `<compounddef kind="namespace"|"file">` blocks whose
//! `<memberdef kind="function">` children are the procs, with `<name>`,
//! `<definition>`, `<argsstring>`, `<briefdescription>`, a
//! `<detaileddescription>` carrying `<parameterlist kind="param">` and
//! `<simplesect kind="return">`, and a `<location file= line=>`. Anything
//! unexpected degrades to a missing field, never a failed load.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One documented proc.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProcDoc {
    /// Qualified, never with a leading `::` -- matches `ctags::TagEntry::name`.
    pub name: String,
    pub brief: String,
    /// The detailed description's running text, paragraphs separated by
    /// blank lines, with the `@param`/`@return` blocks lifted out into
    /// `params`/`returns`.
    pub detail: String,
    pub params: Vec<(String, String)>,
    pub returns: Option<String>,
    /// The argument list as Doxygen saw it (`{name {greeting hello}
    /// args}`) -- a fallback signature for a proc ctags has no entry for.
    pub argsstring: String,
    pub file: PathBuf,
    pub line: usize,
}

/// Every documented proc, by qualified name.
#[derive(Debug, Default)]
pub struct DoxygenIndex {
    docs: HashMap<String, ProcDoc>,
}

impl DoxygenIndex {
    pub fn get(&self, qualified_name: &str) -> Option<&ProcDoc> {
        self.docs.get(qualified_name.trim_start_matches("::"))
    }

    /// The doc for `name` as written at a call site: the qualified name
    /// if it matches, else the first proc whose bare last segment does
    /// (`greet` inside `namespace eval util` for `util::greet`) --
    /// the same resolution `App::user_proc_signature` uses for ctags.
    pub fn resolve(&self, name: &str) -> Option<&ProcDoc> {
        let name = name.trim_start_matches("::");
        if let Some(doc) = self.docs.get(name) {
            return Some(doc);
        }
        let bare = name.rsplit("::").next().unwrap_or(name);
        let mut matches: Vec<&ProcDoc> = self.docs.values().filter(|doc| doc.name.rsplit("::").next() == Some(bare)).collect();
        matches.sort_by(|a, b| a.name.cmp(&b.name));
        matches.into_iter().next()
    }

    pub fn len(&self) -> usize {
        self.docs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &ProcDoc> {
        self.docs.values()
    }
}

/// The `xml_dir` a project's `.fenix/project.ini` names in its
/// `[doxygen]` section, resolved against `root` -- `None` when there's
/// no file, no section, or no key, which is what "this project doesn't
/// use Doxygen" looks like. Same hand-edited-ini reading `fenix-tasks::
/// project_ini` does for `[tasks]`.
pub fn project_xml_dir(root: &Path) -> Option<PathBuf> {
    let contents = std::fs::read_to_string(root.join(".fenix").join("project.ini")).ok()?;
    let mut in_section = false;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_section = header.trim() == "doxygen";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "xml_dir" {
                let value = value.trim();
                if value.is_empty() {
                    return None;
                }
                return Some(root.join(value));
            }
        }
    }
    None
}

/// Reads every documented proc under `xml_dir`. `Err` names what went
/// wrong with the directory itself (no `index.xml`, unparseable) so a
/// manual refresh can report it; a single compound file that fails to
/// parse is skipped, not fatal.
pub fn load(xml_dir: &Path) -> Result<DoxygenIndex, String> {
    let index_path = xml_dir.join("index.xml");
    let index_text = std::fs::read_to_string(&index_path).map_err(|err| format!("couldn't read {}: {err}", index_path.display()))?;
    let index_doc = roxmltree::Document::parse(&index_text).map_err(|err| format!("couldn't parse {}: {err}", index_path.display()))?;
    let mut docs = HashMap::new();
    for compound in index_doc.descendants().filter(|n| n.has_tag_name("compound")) {
        let kind = compound.attribute("kind").unwrap_or("");
        if kind != "namespace" && kind != "file" {
            continue;
        }
        let Some(refid) = compound.attribute("refid") else { continue };
        let path = xml_dir.join(format!("{refid}.xml"));
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Ok(doc) = roxmltree::Document::parse(&text) else { continue };
        for proc_doc in procs_in(&doc) {
            // A proc appears under its namespace compound *and* its
            // file compound; the namespace one carries the qualified
            // name, so it wins -- keep the first entry with the most
            // qualified name.
            docs.entry(proc_doc.name.clone()).or_insert(proc_doc);
        }
    }
    Ok(DoxygenIndex { docs })
}

/// Every `<memberdef kind="function">` in one compound file, as
/// `ProcDoc`s. Public for tests that hand it a document straight from a
/// string.
pub fn procs_in(doc: &roxmltree::Document) -> Vec<ProcDoc> {
    let mut out = Vec::new();
    for compounddef in doc.descendants().filter(|n| n.has_tag_name("compounddef")) {
        let kind = compounddef.attribute("kind").unwrap_or("");
        let compound_name = child_text(compounddef, "compoundname");
        for member in compounddef.descendants().filter(|n| n.has_tag_name("memberdef") && n.attribute("kind") == Some("function")) {
            let bare = child_text(member, "name");
            if bare.is_empty() {
                continue;
            }
            // `<definition>` carries the name as Doxygen resolved it
            // (`util::greet`, sometimes prefixed with the `proc`
            // keyword); trust it when it's qualified, else build the
            // name from the namespace compound.
            let definition = child_text(member, "definition");
            let definition = definition.trim().trim_start_matches("proc ").trim().trim_start_matches("::");
            let name = if definition.contains("::") && !definition.contains(char::is_whitespace) {
                definition.to_string()
            } else if kind == "namespace" && !compound_name.is_empty() {
                format!("{}::{bare}", compound_name.trim_start_matches("::"))
            } else {
                bare.clone()
            };
            let (detail, params, returns) = member
                .children()
                .find(|n| n.has_tag_name("detaileddescription"))
                .map(description_parts)
                .unwrap_or_default();
            let location = member.children().find(|n| n.has_tag_name("location"));
            out.push(ProcDoc {
                name,
                brief: member.children().find(|n| n.has_tag_name("briefdescription")).map(|n| flatten(n).trim().to_string()).unwrap_or_default(),
                detail,
                params,
                returns,
                argsstring: child_text(member, "argsstring").trim().to_string(),
                file: location.and_then(|n| n.attribute("file")).map(PathBuf::from).unwrap_or_default(),
                line: location.and_then(|n| n.attribute("line")).and_then(|l| l.parse().ok()).unwrap_or(0),
            });
        }
    }
    out
}

fn child_text(node: roxmltree::Node, tag: &str) -> String {
    node.children().find(|n| n.has_tag_name(tag)).map(|n| flatten(n)).unwrap_or_default()
}

/// A `<detaileddescription>` split into its running text, its
/// `@param` items and its `@return` note.
fn description_parts(node: roxmltree::Node) -> (String, Vec<(String, String)>, Option<String>) {
    let mut params = Vec::new();
    let mut returns = None;
    for item in node.descendants().filter(|n| n.has_tag_name("parameteritem")) {
        let name = item
            .descendants()
            .find(|n| n.has_tag_name("parametername"))
            .map(|n| flatten(n).trim().to_string())
            .unwrap_or_default();
        let description = item
            .children()
            .find(|n| n.has_tag_name("parameterdescription"))
            .map(|n| flatten(n).trim().to_string())
            .unwrap_or_default();
        if !name.is_empty() {
            params.push((name, description));
        }
    }
    if let Some(section) = node.descendants().find(|n| n.has_tag_name("simplesect") && n.attribute("kind") == Some("return")) {
        let text = flatten(section).trim().to_string();
        if !text.is_empty() {
            returns = Some(text);
        }
    }
    (flatten_prose(node).trim().to_string(), params, returns)
}

/// All text under `node`, paragraphs separated by newlines.
fn flatten(node: roxmltree::Node) -> String {
    let mut out = String::new();
    collect_text(node, &mut out, false);
    out
}

/// Like `flatten`, but leaving out the `<parameterlist>`/`<simplesect>`
/// blocks `description_parts` reports separately, so the running text
/// doesn't repeat them.
fn flatten_prose(node: roxmltree::Node) -> String {
    let mut out = String::new();
    collect_text(node, &mut out, true);
    out
}

fn collect_text(node: roxmltree::Node, out: &mut String, skip_structured: bool) {
    for child in node.children() {
        if child.is_text() {
            out.push_str(child.text().unwrap_or(""));
        } else if child.is_element() {
            if skip_structured && (child.has_tag_name("parameterlist") || child.has_tag_name("simplesect")) {
                continue;
            }
            let is_para = child.has_tag_name("para");
            if is_para && !out.is_empty() && !out.ends_with('\n') {
                out.push_str("\n\n");
            }
            collect_text(child, out, skip_structured);
            if is_para && !out.ends_with('\n') {
                out.push('\n');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMESPACE_XML: &str = r#"<?xml version='1.0' encoding='UTF-8' standalone='no'?>
<doxygen xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" version="1.8.17">
  <compounddef id="namespaceutil" kind="namespace" language="Tcl">
    <compoundname>util</compoundname>
    <sectiondef kind="func">
      <memberdef kind="function" id="namespaceutil_1a1" prot="public" static="no" const="no" explicit="no" inline="no" virt="non-virtual">
        <type></type>
        <definition>util::greet</definition>
        <argsstring>{name {greeting hello} args}</argsstring>
        <name>greet</name>
        <briefdescription>
<para>Greets someone by name. </para>
        </briefdescription>
        <detaileddescription>
<para>Prints the greeting to stdout.</para>
<para>Extra words are appended <emphasis>verbatim</emphasis>.</para>
<para><parameterlist kind="param"><parameteritem>
<parameternamelist>
<parametername>name</parametername>
</parameternamelist>
<parameterdescription>
<para>Who to greet. </para>
</parameterdescription>
</parameteritem>
<parameteritem>
<parameternamelist>
<parametername>greeting</parametername>
</parameternamelist>
<parameterdescription>
<para>The word to lead with. </para>
</parameterdescription>
</parameteritem>
</parameterlist>
<simplesect kind="return"><para>Nothing. </para>
</simplesect>
</para>
        </detaileddescription>
        <inbodydescription>
        </inbodydescription>
        <location file="lib/util.tcl" line="12" column="1" bodyfile="lib/util.tcl" bodystart="12" bodyend="15"/>
      </memberdef>
      <memberdef kind="function" id="namespaceutil_1a2" prot="public" static="no">
        <type></type>
        <definition>proc util::undocumented</definition>
        <argsstring>{}</argsstring>
        <name>undocumented</name>
        <briefdescription>
        </briefdescription>
        <detaileddescription>
        </detaileddescription>
        <location file="lib/util.tcl" line="20" column="1"/>
      </memberdef>
    </sectiondef>
  </compounddef>
</doxygen>
"#;

    const FILE_XML: &str = r#"<?xml version='1.0' encoding='UTF-8' standalone='no'?>
<doxygen version="1.8.17">
  <compounddef id="main_8tcl" kind="file" language="Tcl">
    <compoundname>main.tcl</compoundname>
    <sectiondef kind="func">
      <memberdef kind="function" id="main_8tcl_1a1" prot="public" static="no">
        <type></type>
        <definition>main</definition>
        <argsstring>{argv}</argsstring>
        <name>main</name>
        <briefdescription><para>Entry point. </para></briefdescription>
        <detaileddescription></detaileddescription>
        <location file="main.tcl" line="3" column="1"/>
      </memberdef>
      <memberdef kind="function" id="main_8tcl_1a2" prot="public" static="no">
        <type></type>
        <definition>util::greet</definition>
        <argsstring>{name {greeting hello} args}</argsstring>
        <name>greet</name>
        <briefdescription><para>Greets someone by name. </para></briefdescription>
        <detaileddescription></detaileddescription>
        <location file="lib/util.tcl" line="12" column="1"/>
      </memberdef>
    </sectiondef>
  </compounddef>
</doxygen>
"#;

    const INDEX_XML: &str = r#"<?xml version='1.0' encoding='UTF-8' standalone='no'?>
<doxygenindex version="1.8.17">
  <compound refid="namespaceutil" kind="namespace"><name>util</name>
    <member refid="namespaceutil_1a1" kind="function"><name>greet</name></member>
  </compound>
  <compound refid="main_8tcl" kind="file"><name>main.tcl</name>
    <member refid="main_8tcl_1a1" kind="function"><name>main</name></member>
  </compound>
  <compound refid="classSomething" kind="class"><name>Something</name></compound>
</doxygenindex>
"#;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fenix-doxygen-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_namespace_member_becomes_a_qualified_proc_doc_with_brief_params_and_return() {
        let doc = roxmltree::Document::parse(NAMESPACE_XML).unwrap();
        let procs = procs_in(&doc);
        assert_eq!(procs.len(), 2);
        let greet = &procs[0];
        assert_eq!(greet.name, "util::greet");
        assert_eq!(greet.brief, "Greets someone by name.");
        assert_eq!(greet.detail, "Prints the greeting to stdout.\n\nExtra words are appended verbatim.");
        assert_eq!(greet.params, vec![("name".to_string(), "Who to greet.".to_string()), ("greeting".to_string(), "The word to lead with.".to_string())]);
        assert_eq!(greet.returns.as_deref(), Some("Nothing."));
        assert_eq!(greet.argsstring, "{name {greeting hello} args}");
        assert_eq!(greet.file, PathBuf::from("lib/util.tcl"));
        assert_eq!(greet.line, 12);
    }

    #[test]
    fn a_proc_keyword_in_the_definition_is_not_part_of_the_name() {
        let doc = roxmltree::Document::parse(NAMESPACE_XML).unwrap();
        let procs = procs_in(&doc);
        assert_eq!(procs[1].name, "util::undocumented");
        assert_eq!(procs[1].brief, "");
        assert_eq!(procs[1].returns, None);
        assert!(procs[1].params.is_empty());
    }

    #[test]
    fn a_top_level_proc_in_a_file_compound_keeps_its_bare_name() {
        let doc = roxmltree::Document::parse(FILE_XML).unwrap();
        let procs = procs_in(&doc);
        assert_eq!(procs[0].name, "main");
        assert_eq!(procs[1].name, "util::greet", "a qualified definition wins over the file compound's name");
    }

    #[test]
    fn load_reads_every_namespace_and_file_compound_the_index_lists_once() {
        let dir = temp_dir("load");
        std::fs::write(dir.join("index.xml"), INDEX_XML).unwrap();
        std::fs::write(dir.join("namespaceutil.xml"), NAMESPACE_XML).unwrap();
        std::fs::write(dir.join("main_8tcl.xml"), FILE_XML).unwrap();

        let index = load(&dir).unwrap();

        assert_eq!(index.len(), 3, "greet appears under both its namespace and its file, counted once");
        assert_eq!(index.get("util::greet").unwrap().params.len(), 2, "the namespace compound's fuller entry wins");
        assert_eq!(index.get("::util::greet").unwrap().name, "util::greet");
        assert_eq!(index.get("main").unwrap().brief, "Entry point.");
        assert!(index.get("Something").is_none(), "a class compound isn't opened");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_reports_a_missing_index_rather_than_pretending_the_project_is_undocumented() {
        let dir = temp_dir("missing");
        let err = load(&dir).unwrap_err();
        assert!(err.contains("index.xml"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_falls_back_to_the_bare_name() {
        let doc = roxmltree::Document::parse(NAMESPACE_XML).unwrap();
        let index = DoxygenIndex { docs: procs_in(&doc).into_iter().map(|d| (d.name.clone(), d)).collect() };
        assert_eq!(index.resolve("util::greet").unwrap().name, "util::greet");
        assert_eq!(index.resolve("greet").unwrap().name, "util::greet");
        assert_eq!(index.resolve("::util::greet").unwrap().name, "util::greet");
        assert!(index.resolve("nothing").is_none());
    }

    #[test]
    fn project_xml_dir_reads_the_doxygen_section_of_project_ini() {
        let dir = temp_dir("ini");
        std::fs::create_dir_all(dir.join(".fenix")).unwrap();
        std::fs::write(dir.join(".fenix").join("project.ini"), "[tasks]\ntask1 = build|make\n\n[doxygen]\n# where GENERATE_XML writes\nxml_dir = docs/xml\n").unwrap();
        assert_eq!(project_xml_dir(&dir), Some(dir.join("docs/xml")));

        std::fs::write(dir.join(".fenix").join("project.ini"), "[tasks]\ntask1 = build|make\n").unwrap();
        assert_eq!(project_xml_dir(&dir), None, "no section means the project doesn't use Doxygen");

        std::fs::write(dir.join(".fenix").join("project.ini"), "[doxygen]\nxml_dir =\n").unwrap();
        assert_eq!(project_xml_dir(&dir), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
