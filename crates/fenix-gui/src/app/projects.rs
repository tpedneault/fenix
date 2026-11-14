//! Projects in the chrome: the kind tag and name the modeline, the
//! window title and Home put in front of a project, all in one colour
//! per kind so the tag reads as the same mark everywhere. The project
//! pages themselves (wizard, hub, doctor, settings) are in `pages`.

use super::*;
use fenix_project::{KindColor, ProjectKind};

/// A kind's colour, taken from the theme's own syntax roles rather than
/// fixed values, so a tag stays readable on a light theme too. A kind
/// names its role (`KindColor`); a kind no one declares is muted, as
/// Other is.
pub(super) fn kind_color(kind: ProjectKind, theme: &Theme) -> glyphon::Color {
    match kind.color() {
        KindColor::Syntax(role) => match role {
            "keyword" => theme.syntax_keyword,
            "string" => theme.syntax_string,
            "function" => theme.syntax_function,
            "type" => theme.syntax_type,
            "number" => theme.syntax_number,
            "attribute" => theme.syntax_attribute,
            "variable" => theme.syntax_variable,
            _ => theme.syntax_constant,
        },
        KindColor::Insert => rgba_to_glyphon(theme.mode_insert),
        KindColor::Fg => theme.fg,
        KindColor::Muted => theme.gutter_fg,
    }
}

/// `PY orbit-tools · ` -- the modeline's project segment, drawn between
/// the mode label and the file name.
pub(super) fn modeline_project_spans(kind: ProjectKind, name: &str, theme: &Theme) -> Vec<(String, glyphon::Color)> {
    vec![
        (format!("{} ", kind.tag()), kind_color(kind, theme)),
        (name.to_string(), theme.fg_modeline),
        (" · ".to_string(), theme.gutter_fg),
    ]
}

/// "orbit-tools — Fenix 1.3.0", or "Fenix 1.3.0" outside a project -- so
/// the taskbar tells two Fenix windows apart, and says which Fenix.
pub(crate) fn window_title(project: Option<&str>) -> String {
    let fenix = format!("Fenix {}", crate::installed::RELEASE);
    match project {
        Some(name) => format!("{name} \u{2014} {fenix}"),
        None => fenix,
    }
}

impl App {
    /// Keeps the OS window's title on the focused buffer's project.
    /// Compared first: setting a title is a system call and a repaint
    /// of the title bar, and this runs every frame.
    pub(super) fn sync_window_title(&self) {
        let Some(window) = &self.window else { return };
        let name = self.project_root.as_deref().and_then(|root| root.file_name()).map(|n| n.to_string_lossy().into_owned());
        let title = window_title(name.as_deref());
        if window.title() != title {
            window.set_title(&title);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_modeline_segment_is_the_tag_then_the_name() {
        let spans = modeline_project_spans(ProjectKind::ARDUINO, "blink-lab", theme::by_name("Visual Studio Dark").unwrap());
        let text: String = spans.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text, "INO blink-lab · ");
        assert_eq!(spans[0].1, rgba_to_glyphon(theme::by_name("Visual Studio Dark").unwrap().mode_insert));
    }

    /// Fenix's kinds keep the colours they had, and a kind no one
    /// declares is drawn as Other is.
    #[test]
    fn each_kind_keeps_its_colour() {
        let theme = theme::by_name("Visual Studio Dark").unwrap();
        let expected = [
            (ProjectKind::PYTHON, theme.syntax_type),
            (ProjectKind::ARDUINO, rgba_to_glyphon(theme.mode_insert)),
            (ProjectKind::RUST, theme.syntax_string),
            (ProjectKind::TCL, theme.syntax_function),
            (ProjectKind::CPP, theme.syntax_keyword),
            (ProjectKind::NODE, theme.syntax_number),
            (ProjectKind::GO, theme.syntax_type),
            (ProjectKind::MONOREPO, theme.fg),
            (ProjectKind::OTHER, theme.gutter_fg),
        ];
        for (kind, color) in expected {
            assert_eq!(kind_color(kind, theme), color, "{kind:?}");
        }
        assert_eq!(kind_color(ProjectKind::from_id("cobol").unwrap(), theme), theme.gutter_fg);
    }

    #[test]
    fn the_window_is_named_after_its_project() {
        let fenix = format!("Fenix {}", crate::installed::RELEASE);
        assert_eq!(window_title(Some("orbit-tools")), format!("orbit-tools \u{2014} {fenix}"));
        assert_eq!(window_title(None), fenix);
    }
}
