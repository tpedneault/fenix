//! Which colours a diagram is drawn in. In order: what its source asks
//! for (`config: theme:` front matter or a `%%{init}%%` directive, plus
//! any `themeVariables`), else the default (`diagrams.theme`). A theme is
//! one of Mermaid's (`default`, `neutral`, `dark`, `forest`, `base`),
//! `fenix` -- the editor theme's colours -- or one of yours: variables
//! over a theme to start from.

use mermaid_rs_renderer::Theme;

/// The editor theme's colours `fenix` is made from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorColors {
    pub bg: [u8; 3],
    pub fg: [u8; 3],
    /// Panels and popups: node fills.
    pub panel: [u8; 3],
    /// The caret's colour: borders.
    pub accent: [u8; 3],
    /// Muted text: lines and arrows.
    pub muted: [u8; 3],
    /// The current line's tint: second fills, clusters.
    pub tint: [u8; 3],
    pub warn: [u8; 3],
}

impl Default for EditorColors {
    /// Visual Studio Dark's.
    fn default() -> Self {
        EditorColors { bg: [0x1e, 0x1e, 0x1e], fg: [0xd4, 0xd4, 0xd4], panel: [0x25, 0x25, 0x26], accent: [0x56, 0x9c, 0xd6], muted: [0x9c, 0x9c, 0x9c], tint: [0x2d, 0x2d, 0x30], warn: [0xff, 0x6a, 0x3d] }
    }
}

/// One of your themes: a name, what it starts from, and its variables.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThemeSpec {
    pub name: String,
    pub base: String,
    pub vars: Vec<(String, String)>,
}

/// Everything a diagram's theme can come from.
#[derive(Debug, Clone, PartialEq)]
pub struct Themes {
    /// The theme a diagram that names none is drawn in.
    pub default: String,
    pub custom: Vec<ThemeSpec>,
    pub editor: EditorColors,
    /// Replaces every theme's font, when set.
    pub font: Option<String>,
}

impl Default for Themes {
    fn default() -> Self {
        Themes { default: "default".into(), custom: Vec::new(), editor: EditorColors::default(), font: None }
    }
}

pub const BUILTIN: &[&str] = &["default", "neutral", "dark", "forest", "base"];

pub fn is_builtin(name: &str) -> bool {
    BUILTIN.iter().any(|b| b.eq_ignore_ascii_case(name))
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    [0, 1, 2].map(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8)
}

/// Mermaid's own themes (and this renderer's `modern`).
pub fn builtin(name: &str) -> Option<Theme> {
    match name.trim().to_ascii_lowercase().as_str() {
        "default" | "mermaid" => Some(Theme::mermaid_default()),
        "neutral" => Some(Theme::neutral()),
        "dark" => Some(Theme::dark()),
        "forest" => Some(Theme::forest()),
        "modern" => Some(Theme::modern()),
        "base" => {
            // Mermaid's base: the warm starting point themeVariables tune.
            let mut t = Theme::mermaid_default();
            apply(&mut t, [("primaryColor", "#fff4dd"), ("primaryBorderColor", "#9f8a54"), ("secondaryColor", "#ffe7b3"), ("tertiaryColor", "#fff9ec"), ("lineColor", "#333333"), ("clusterBkg", "#fff9ec"), ("clusterBorder", "#c9a34a")].iter().map(|(k, v)| (*k, *v)));
            Some(t)
        }
        _ => None,
    }
}

/// The `fenix` theme's variables, from the editor's colours.
fn fenix_vars(e: &EditorColors) -> Vec<(String, String)> {
    let dark = e.bg.iter().map(|&c| c as u32).sum::<u32>() < 384;
    let fill = if dark { mix(e.bg, e.panel, 1.0) } else { mix(e.panel, e.bg, 0.3) };
    let fill = if fill == e.bg { mix(e.bg, e.fg, 0.08) } else { fill };
    let second = mix(e.bg, e.accent, 0.18);
    [
        ("background", hex(e.bg)),
        ("primaryColor", hex(fill)),
        ("primaryTextColor", hex(e.fg)),
        ("primaryBorderColor", hex(e.accent)),
        ("lineColor", hex(e.muted)),
        ("secondaryColor", hex(second)),
        ("tertiaryColor", hex(mix(e.bg, e.tint, 1.0))),
        ("textColor", hex(e.fg)),
        ("edgeLabelBackground", hex(e.bg)),
        ("clusterBkg", hex(mix(e.bg, e.tint, 0.7))),
        ("clusterBorder", hex(e.muted)),
        ("noteBkgColor", hex(mix(e.bg, e.warn, 0.15))),
        ("noteBorderColor", hex(e.warn)),
        ("actorBkg", hex(fill)),
        ("actorBorder", hex(e.accent)),
        ("actorLineColor", hex(e.muted)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// Sets Mermaid theme variables on `t`.
pub fn apply<'a>(t: &mut Theme, vars: impl Iterator<Item = (&'a str, &'a str)>) {
    for (key, value) in vars {
        let v = value.trim().trim_matches('"').to_string();
        if v.is_empty() {
            continue;
        }
        match key {
            "primaryColor" => t.primary_color = v,
            "primaryTextColor" => {
                t.primary_text_color = v.clone();
                t.pie_title_text_color = v.clone();
                t.pie_legend_text_color = v;
            }
            "primaryBorderColor" => t.primary_border_color = v,
            "lineColor" => t.line_color = v,
            "secondaryColor" => t.secondary_color = v,
            "tertiaryColor" => t.tertiary_color = v,
            "textColor" => t.text_color = v,
            "edgeLabelBackground" => t.edge_label_background = v,
            "clusterBkg" => t.cluster_background = v,
            "clusterBorder" => t.cluster_border = v,
            "background" => t.background = v,
            "noteBkgColor" => t.sequence_note_fill = v,
            "noteBorderColor" => t.sequence_note_border = v,
            "actorBkg" => t.sequence_actor_fill = v,
            "actorBorder" => t.sequence_actor_border = v,
            "actorLineColor" | "signalColor" => t.sequence_actor_line = v,
            "activationBkgColor" => t.sequence_activation_fill = v,
            "activationBorderColor" => t.sequence_activation_border = v,
            "fontFamily" => t.font_family = v,
            "fontSize" => {
                if let Ok(n) = v.trim_end_matches("px").parse::<f32>() {
                    t.font_size = n;
                }
            }
            k if k.starts_with("pie") => {
                if let Ok(n) = k[3..].parse::<usize>() {
                    if (1..=12).contains(&n) {
                        t.pie_colors[n - 1] = v;
                    }
                }
            }
            _ => {}
        }
    }
}

/// What a source's front matter asks: its theme and variables.
fn front_matter(source: &str) -> (Option<String>, Vec<(String, String)>) {
    let lines: Vec<&str> = source.lines().collect();
    if lines.first().map(|l| l.trim()) != Some("---") {
        return (None, Vec::new());
    }
    let Some(end) = lines.iter().skip(1).position(|l| l.trim() == "---").map(|e| e + 1) else { return (None, Vec::new()) };
    let mut theme = None;
    let mut vars = Vec::new();
    let mut in_vars: Option<usize> = None;
    for line in &lines[1..end] {
        let indent = line.len() - line.trim_start().len();
        let t = line.trim();
        if let Some(level) = in_vars {
            if indent > level {
                if let Some((k, v)) = t.split_once(':') {
                    vars.push((k.trim().to_string(), v.trim().trim_matches(['"', '\'']).to_string()));
                }
                continue;
            }
            in_vars = None;
        }
        if let Some(v) = t.strip_prefix("theme:") {
            theme = Some(v.trim().trim_matches(['"', '\'']).to_string()).filter(|v| !v.is_empty());
        } else if t == "themeVariables:" {
            in_vars = Some(indent);
        }
    }
    (theme, vars)
}

impl Themes {
    /// The theme `source` is drawn in, by name.
    pub fn name_for(&self, source: &str, init: Option<&serde_json::Value>) -> String {
        let (fm, _) = front_matter(source);
        fm.or_else(|| init.and_then(|i| i.get("theme")).and_then(|v| v.as_str()).map(str::to_string)).unwrap_or_else(|| self.default.clone())
    }

    /// Every theme there is, by name: Mermaid's, `fenix`, yours.
    pub fn names(&self) -> Vec<String> {
        let mut out: Vec<String> = BUILTIN.iter().map(|s| s.to_string()).collect();
        out.push("fenix".into());
        out.extend(self.custom.iter().map(|c| c.name.clone()));
        out
    }

    /// A theme's variables, as they'd be written out: `fenix`'s colours,
    /// or one of yours over what it starts from.
    pub fn variables_of(&self, name: &str) -> Vec<(String, String)> {
        if name.eq_ignore_ascii_case("fenix") {
            return fenix_vars(&self.editor);
        }
        match self.custom.iter().find(|c| c.name.eq_ignore_ascii_case(name)) {
            Some(spec) => {
                let mut vars = if spec.base.eq_ignore_ascii_case("fenix") { fenix_vars(&self.editor) } else { Vec::new() };
                for (k, v) in &spec.vars {
                    if v.trim().is_empty() {
                        continue;
                    }
                    match vars.iter_mut().find(|(x, _)| x == k) {
                        Some(slot) => slot.1 = v.clone(),
                        None => vars.push((k.clone(), v.clone())),
                    }
                }
                vars
            }
            None => Vec::new(),
        }
    }

    /// A theme by name, before a source's own variables.
    pub fn theme(&self, name: &str) -> Theme {
        if let Some(t) = builtin(name) {
            return t;
        }
        let mut t = match self.custom.iter().find(|c| c.name.eq_ignore_ascii_case(name)) {
            Some(spec) if !spec.base.eq_ignore_ascii_case("fenix") => builtin(&spec.base).unwrap_or_else(Theme::mermaid_default),
            _ => builtin("base").unwrap_or_else(Theme::mermaid_default),
        };
        let vars = self.variables_of(if name.eq_ignore_ascii_case("fenix") || self.custom.iter().any(|c| c.name.eq_ignore_ascii_case(name)) { name } else { "fenix" });
        apply(&mut t, vars.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        t
    }

    /// The theme `source` is drawn in (`over` wins when given), with the
    /// source's own `themeVariables`, and its name.
    pub fn resolve(&self, source: &str, init: Option<&serde_json::Value>, over: Option<&str>) -> (Theme, String) {
        let name = over.map(str::to_string).unwrap_or_else(|| self.name_for(source, init));
        let mut t = self.theme(&name);
        let (_, fm_vars) = front_matter(source);
        apply(&mut t, fm_vars.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        if let Some(obj) = init.and_then(|i| i.get("themeVariables")).and_then(|v| v.as_object()) {
            let pairs: Vec<(String, String)> = obj.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())).or_else(|| v.as_f64().map(|n| (k.clone(), n.to_string())))).collect();
            apply(&mut t, pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        }
        if let Some(font) = &self.font {
            t.font_family = font.clone();
        }
        (t, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_theme_and_variables() {
        let (t, v) = front_matter("---\nconfig:\n  theme: forest\n  themeVariables:\n    primaryColor: \"#ff0000\"\n    lineColor: '#00ff00'\n---\nflowchart TD\n");
        assert_eq!(t.as_deref(), Some("forest"));
        assert_eq!(v, vec![("primaryColor".into(), "#ff0000".into()), ("lineColor".into(), "#00ff00".into())]);
    }

    #[test]
    fn fenix_follows_the_editor_and_yours_start_from_a_theme() {
        let mut themes = Themes::default();
        let fenix = themes.theme("fenix");
        assert_eq!(fenix.background, "#1e1e1e");
        assert_eq!(fenix.primary_border_color, "#569cd6");
        themes.editor.accent = [0xff, 0, 0];
        assert_eq!(themes.theme("fenix").primary_border_color, "#ff0000");
        themes.custom.push(ThemeSpec { name: "mission".into(), base: "dark".into(), vars: vec![("primaryColor".into(), "#16283a".into()), ("lineColor".into(), "".into())] });
        let m = themes.theme("mission");
        assert_eq!(m.primary_color, "#16283a");
        assert_eq!(m.line_color, Theme::dark().line_color, "an empty variable keeps the theme's");
        assert!(themes.names().contains(&"mission".to_string()));
        let (t, name) = themes.resolve("---\nconfig:\n  themeVariables:\n    primaryColor: \"#123456\"\n---\nflowchart TD\n", None, Some("forest"));
        assert_eq!(name, "forest");
        assert_eq!(t.primary_color, "#123456");
    }
}
