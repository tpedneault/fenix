use fenix_docker::{Container, Image};

/// What a docker-panel line represents -- what `s`/`S`/`R`/`r`/`x` act on
/// when the cursor is on this line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockerEntry {
    Container(String),
    Image(String),
}

/// How one generated line should be colored -- same role as
/// `dashboard::DashboardLineStyle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockerLineStyle {
    Header,
    Container,
    Image,
    Footer,
    /// Shown only when both lists come back empty -- no `docker` on
    /// `PATH`, an unreachable daemon, or genuinely nothing to show.
    Empty,
}

/// Per-line metadata for one line of `DockerPanel::text`, at the matching
/// index in `DockerPanel::lines` -- mirrors `dashboard::DashboardLine`.
#[derive(Debug, Clone)]
pub struct DockerLine {
    pub style: DockerLineStyle,
    /// `Some` only for a `Container`/`Image` row -- what the action keys
    /// (`s`/`S`/`R`/`r`/`x`) target when the cursor is on this line.
    pub entry: Option<DockerEntry>,
    /// Char column where the dim (status/size) portion of the line
    /// begins. `None` for every other style.
    pub dim_from: Option<usize>,
}

/// The generated docker panel: `text` is real content for a real
/// `fenix_core::Buffer` (via `BufferList::open_docker`); `lines[i]`
/// describes `text`'s line `i`, mirroring `dashboard::Dashboard` exactly.
pub struct DockerPanel {
    pub text: String,
    pub lines: Vec<Option<DockerLine>>,
}

struct Builder {
    text: String,
    lines: Vec<Option<DockerLine>>,
}

impl Builder {
    fn new() -> Self {
        Self { text: String::new(), lines: Vec::new() }
    }

    fn push(&mut self, text: &str, meta: Option<DockerLine>) {
        self.text.push_str(text);
        self.text.push('\n');
        self.lines.push(meta);
    }

    fn blank(&mut self) {
        self.push("", None);
    }

    fn finish(self) -> DockerPanel {
        DockerPanel { text: self.text, lines: self.lines }
    }
}

/// Builds the Docker management panel shown by `SPC d d`: a "Containers"
/// section (state, name, image, status), an "Images" section (repository:
/// tag, size), and a footer hint line -- Lazydocker-style, minus the log
/// streaming/CPU graphs (out of scope, see the plan). Either section is
/// omitted entirely when its list is empty, same convention
/// `dashboard::render` already established for projects/recent files;
/// if *both* are empty (no `docker` on `PATH`, daemon unreachable, or
/// genuinely nothing to show), a single explanatory line takes their
/// place instead of two silently-blank sections.
pub fn render(containers: &[Container], images: &[Image]) -> DockerPanel {
    let mut b = Builder::new();
    b.push("  Docker", Some(DockerLine { style: DockerLineStyle::Header, entry: None, dim_from: None }));
    b.blank();

    if containers.is_empty() && images.is_empty() {
        b.push(
            "    No containers or images found -- is docker installed and running?",
            Some(DockerLine { style: DockerLineStyle::Empty, entry: None, dim_from: None }),
        );
        b.blank();
    } else {
        push_containers(&mut b, containers);
        push_images(&mut b, images);
    }

    push_footer(&mut b);
    b.finish()
}

fn push_containers(b: &mut Builder, containers: &[Container]) {
    if containers.is_empty() {
        return;
    }
    b.push("  Containers", Some(DockerLine { style: DockerLineStyle::Header, entry: None, dim_from: None }));
    for c in containers {
        let prefix = format!("    {}  ", c.name);
        let dim_from = prefix.chars().count();
        let line = format!("{prefix}{}  {}", c.image, c.status);
        b.push(
            &line,
            Some(DockerLine {
                style: DockerLineStyle::Container,
                entry: Some(DockerEntry::Container(c.id.clone())),
                dim_from: Some(dim_from),
            }),
        );
    }
    b.blank();
}

fn push_images(b: &mut Builder, images: &[Image]) {
    if images.is_empty() {
        return;
    }
    b.push("  Images", Some(DockerLine { style: DockerLineStyle::Header, entry: None, dim_from: None }));
    for img in images {
        let name = if img.tag.is_empty() { img.repository.clone() } else { format!("{}:{}", img.repository, img.tag) };
        let prefix = format!("    {name}  ");
        let dim_from = prefix.chars().count();
        let line = format!("{prefix}{}", img.size);
        b.push(
            &line,
            Some(DockerLine {
                style: DockerLineStyle::Image,
                entry: Some(DockerEntry::Image(img.id.clone())),
                dim_from: Some(dim_from),
            }),
        );
    }
    b.blank();
}

fn push_footer(b: &mut Builder) {
    b.push(
        "  s start  S stop  R restart  r run image  l logs  x remove  u refresh",
        Some(DockerLine { style: DockerLineStyle::Footer, entry: None, dim_from: None }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn container(id: &str, name: &str) -> Container {
        Container { id: id.to_string(), name: name.to_string(), image: "nginx:latest".to_string(), status: "Up 3 days".to_string(), state: "running".to_string() }
    }

    fn image(id: &str, repo: &str, tag: &str) -> Image {
        Image { id: id.to_string(), repository: repo.to_string(), tag: tag.to_string(), size: "142MB".to_string() }
    }

    #[test]
    fn text_and_lines_always_stay_the_same_length() {
        let panel = render(&[container("a", "web")], &[image("b", "nginx", "latest")]);
        assert_eq!(panel.text.lines().count(), panel.lines.len());
    }

    #[test]
    fn empty_state_shows_a_single_explanatory_line_not_two_blank_sections() {
        let panel = render(&[], &[]);
        assert!(panel.text.contains("is docker installed and running?"));
        assert!(!panel.text.contains("Containers"));
        assert!(!panel.text.contains("Images"));
    }

    #[test]
    fn containers_are_listed_with_the_right_entry_at_the_right_line() {
        let containers = vec![container("id1", "web"), container("id2", "db")];
        let panel = render(&containers, &[]);
        let header_line = panel.text.lines().position(|l| l.trim() == "Containers").unwrap();
        let first = panel.lines[header_line + 1].as_ref().unwrap();
        assert_eq!(first.style, DockerLineStyle::Container);
        assert_eq!(first.entry, Some(DockerEntry::Container("id1".to_string())));
        let second = panel.lines[header_line + 2].as_ref().unwrap();
        assert_eq!(second.entry, Some(DockerEntry::Container("id2".to_string())));
    }

    #[test]
    fn images_are_listed_with_the_right_entry_at_the_right_line() {
        let images = vec![image("id1", "nginx", "latest"), image("id2", "redis", "7")];
        let panel = render(&[], &images);
        let header_line = panel.text.lines().position(|l| l.trim() == "Images").unwrap();
        let first = panel.lines[header_line + 1].as_ref().unwrap();
        assert_eq!(first.style, DockerLineStyle::Image);
        assert_eq!(first.entry, Some(DockerEntry::Image("id1".to_string())));
    }

    #[test]
    fn image_with_no_tag_omits_the_trailing_colon() {
        let images = vec![image("id1", "myimage", "")];
        let panel = render(&[], &images);
        assert!(panel.text.contains("myimage  "));
        assert!(!panel.text.contains("myimage:"));
    }

    #[test]
    fn containers_section_omitted_when_only_images_present() {
        let panel = render(&[], &[image("id1", "nginx", "latest")]);
        assert!(!panel.text.contains("Containers"));
        assert!(panel.text.contains("Images"));
    }

    #[test]
    fn footer_hint_line_is_always_present() {
        let panel = render(&[], &[]);
        let has_footer = panel.lines.iter().flatten().any(|l| l.style == DockerLineStyle::Footer);
        assert!(has_footer);
    }
}
