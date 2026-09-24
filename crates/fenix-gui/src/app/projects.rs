//! Projects in the chrome: the kind tag and name the modeline, the
//! window title and Home put in front of a project, all in one colour
//! per kind so the tag reads as the same mark everywhere. The project
//! pages themselves (wizard, hub, doctor, settings) are in `pages`.

use super::*;
use fenix_project::ProjectKind;

/// A kind's colour, taken from the theme's own syntax roles rather than
/// fixed values, so a tag stays readable on a light theme too.
pub(super) fn kind_color(kind: ProjectKind, theme: &Theme) -> glyphon::Color {
    match kind {
        ProjectKind::Python => theme.syntax_type,
        ProjectKind::Arduino => rgba_to_glyphon(theme.mode_insert),
        ProjectKind::Mib => theme.syntax_constant,
        ProjectKind::Rust => theme.syntax_string,
        ProjectKind::Tcl => theme.syntax_function,
        ProjectKind::Cpp => theme.syntax_keyword,
        ProjectKind::Node => theme.syntax_number,
        ProjectKind::Go => theme.syntax_type,
        ProjectKind::Monorepo => theme.fg,
        ProjectKind::Other => theme.gutter_fg,
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

/// "orbit-tools — Fenix", or plain "Fenix" outside a project -- so the
/// taskbar tells two Fenix windows apart.
pub(super) fn window_title(project: Option<&str>) -> String {
    match project {
        Some(name) => format!("{name} \u{2014} Fenix"),
        None => "Fenix".to_string(),
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
        let spans = modeline_project_spans(ProjectKind::Arduino, "blink-lab", &theme::VISUAL_STUDIO_DARK);
        let text: String = spans.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text, "INO blink-lab · ");
        assert_eq!(spans[0].1, rgba_to_glyphon(theme::VISUAL_STUDIO_DARK.mode_insert));
    }

    #[test]
    fn the_window_is_named_after_its_project() {
        assert_eq!(window_title(Some("orbit-tools")), "orbit-tools \u{2014} Fenix");
        assert_eq!(window_title(None), "Fenix");
    }
}
