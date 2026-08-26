use fenix_docker::{Container, ContainerStat, Image, Network, Volume};

/// What a docker-panel line represents -- what the per-pane action keys
/// (`s`/`S`/`R`/`r`/`d`, see `app.rs`'s Docker action routing) act on
/// when the cursor is on this line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockerEntry {
    Container(String),
    Image(String),
    Volume(String),
    /// Keyed by the network's `id`, like `Container`/`Image` -- unlike
    /// `Volume`, a `Network` has a real distinct id field, not just a
    /// name.
    Network(String),
}

/// How one generated line should be colored -- same role as
/// `dashboard::DashboardLineStyle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockerLineStyle {
    Container,
    Image,
    Volume,
    Network,
    /// A `label: value` row in the Status pane -- no `entry` (nothing to
    /// act on), just a dimmable label prefix like `Container`/`Image`
    /// rows already have.
    Detail,
    /// Shown when a list comes back empty -- no `docker` on `PATH`, an
    /// unreachable daemon, or genuinely nothing to show (also reused
    /// for the Status pane's "nothing selected" placeholder).
    Empty,
}

/// A coarse "how's this doing" bucket for a container's status badge --
/// kept theme-agnostic here (this crate has no `Theme` dependency; see
/// `app.rs`'s `docker_highlights_for_visible_range` for the actual
/// color mapping), the same "tag the meaning, let the host pick the
/// color" split `DockerLineStyle` already uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockerBadgeColor {
    Good,
    Warn,
    Bad,
    Neutral,
}

/// A single-letter badge + its color bucket for a container's `State`
/// field -- verified against Docker's own documented container state
/// machine (created, running, paused, restarting, exited, dead,
/// removing; https://docs.docker.com/reference/cli/docker/container/ls/),
/// not guessed. Any other/unrecognized value (including an empty
/// string) falls back to "N" (none) rather than guessing at a letter.
fn container_status_badge(state: &str) -> (&'static str, DockerBadgeColor) {
    match state {
        "running" => ("R", DockerBadgeColor::Good),
        "paused" => ("P", DockerBadgeColor::Warn),
        "restarting" => ("~", DockerBadgeColor::Warn),
        "created" => ("C", DockerBadgeColor::Neutral),
        "exited" => ("X", DockerBadgeColor::Bad),
        "dead" => ("D", DockerBadgeColor::Bad),
        "removing" => ("V", DockerBadgeColor::Bad),
        _ => ("N", DockerBadgeColor::Neutral),
    }
}

/// What the Status pane is currently describing -- one of the three
/// listable resource kinds, each holding the same struct `fenix_docker`
/// already produces (no separate "detail" data model to keep in sync).
pub enum DockerDetail {
    Container(Container),
    Image(Image),
    Volume(Volume),
    Network(Network),
}

/// Per-line metadata for one line of `DockerPanel::text`, at the matching
/// index in `DockerPanel::lines` -- mirrors `dashboard::DashboardLine`.
#[derive(Debug, Clone)]
pub struct DockerLine {
    pub style: DockerLineStyle,
    /// `Some` only for a `Container`/`Image`/`Volume` row -- what the
    /// action keys target when the cursor is on this line.
    pub entry: Option<DockerEntry>,
    /// Char column where the dim (status/size) portion of the line
    /// begins. `None` for every other style.
    pub dim_from: Option<usize>,
    /// A Containers-pane row's `[X]` status prefix: its char length
    /// (so the range `0..len` can be colored) and which color bucket
    /// to use. `None` for every other row -- only Containers rows have
    /// a status badge at all.
    pub badge: Option<(usize, DockerBadgeColor)>,
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

    fn finish(self) -> DockerPanel {
        DockerPanel { text: self.text, lines: self.lines }
    }
}

/// The Containers pane's own content -- no in-buffer header (the pane's
/// own title bar already says "Containers") and, deliberately, nothing
/// but a colored one-letter status badge plus the name: image/status/
/// live stats all used to be crammed onto this same row and routinely
/// got clipped by the pane's real-world width, so that information
/// moved to the Status pane instead (see `render_details`) -- this row
/// only needs to answer "which one, and is it healthy" at a glance.
/// The action-key footer hint that used to live here is gone too, for
/// the same reason -- see `x`'s new "show this pane's keys" menu in
/// `app.rs` (`docker_menu_popup`) instead of a line that competed with
/// real content for the same clipped width.
pub fn render_containers(containers: &[Container]) -> DockerPanel {
    let mut b = Builder::new();
    if containers.is_empty() {
        b.push("    No containers found", Some(DockerLine { style: DockerLineStyle::Empty, entry: None, dim_from: None, badge: None }));
    } else {
        for c in containers {
            let (letter, color) = container_status_badge(&c.state);
            let prefix = format!("  [{letter}] ");
            let badge_len = prefix.chars().count();
            let line = format!("{prefix}{}", c.name);
            b.push(
                &line,
                Some(DockerLine {
                    style: DockerLineStyle::Container,
                    entry: Some(DockerEntry::Container(c.id.clone())),
                    dim_from: None,
                    badge: Some((badge_len, color)),
                }),
            );
        }
    }
    b.finish()
}

/// Tolerant match between a cached `Container::id` and a `docker stats`
/// row's own `ID` field -- both are documented as short-form container
/// IDs, so exact equality is the common case, but a prefix match either
/// direction is kept as a fallback in case a given engine/version ever
/// returns them at different lengths. `pub`: also used by `app.rs` to
/// find the currently-selected container's own stat row for the Status
/// pane (see `render_details`'s `stat` parameter).
pub fn find_stat<'a>(stats: &'a [ContainerStat], id: &str) -> Option<&'a ContainerStat> {
    stats.iter().find(|s| s.container_id == id || id.starts_with(&s.container_id) || s.container_id.starts_with(id))
}

/// The Images pane's own content -- same "no in-buffer header, no
/// footer hint line" reasoning as `render_containers`.
pub fn render_images(images: &[Image]) -> DockerPanel {
    let mut b = Builder::new();
    if images.is_empty() {
        b.push("    No images found", Some(DockerLine { style: DockerLineStyle::Empty, entry: None, dim_from: None, badge: None }));
    } else {
        for img in images {
            let name = if img.tag.is_empty() { img.repository.clone() } else { format!("{}:{}", img.repository, img.tag) };
            let prefix = format!("  {name}  ");
            let dim_from = prefix.chars().count();
            let line = format!("{prefix}{}", img.size);
            b.push(
                &line,
                Some(DockerLine {
                    style: DockerLineStyle::Image,
                    entry: Some(DockerEntry::Image(img.id.clone())),
                    dim_from: Some(dim_from),
                    badge: None,
                }),
            );
        }
    }
    b.finish()
}

/// The Volumes pane's own content -- list-only (see the plan's Scope:
/// no `fenix_docker` volume-remove function exists yet, so there's
/// nothing destructive to bind here beyond refresh).
pub fn render_volumes(volumes: &[Volume]) -> DockerPanel {
    let mut b = Builder::new();
    if volumes.is_empty() {
        b.push("    No volumes found", Some(DockerLine { style: DockerLineStyle::Empty, entry: None, dim_from: None, badge: None }));
    } else {
        for v in volumes {
            let prefix = format!("  {}  ", v.name);
            let dim_from = prefix.chars().count();
            let line = format!("{prefix}{}  {}", v.driver, v.mountpoint);
            b.push(
                &line,
                Some(DockerLine {
                    style: DockerLineStyle::Volume,
                    entry: Some(DockerEntry::Volume(v.name.clone())),
                    dim_from: Some(dim_from),
                    badge: None,
                }),
            );
        }
    }
    b.finish()
}

/// The Networks pane's own content -- same "no in-buffer header" shape
/// as `render_volumes`, but unlike Volumes, `d` genuinely removes here
/// (see `fenix_docker::remove_network`).
pub fn render_networks(networks: &[Network]) -> DockerPanel {
    let mut b = Builder::new();
    if networks.is_empty() {
        b.push("    No networks found", Some(DockerLine { style: DockerLineStyle::Empty, entry: None, dim_from: None, badge: None }));
    } else {
        for n in networks {
            let prefix = format!("  {}  ", n.name);
            let dim_from = prefix.chars().count();
            let line = format!("{prefix}{}  {}", n.driver, n.scope);
            b.push(
                &line,
                Some(DockerLine {
                    style: DockerLineStyle::Network,
                    entry: Some(DockerEntry::Network(n.id.clone())),
                    dim_from: Some(dim_from),
                    badge: None,
                }),
            );
        }
    }
    b.finish()
}

/// The Status pane's own content: plain `label: value` rows from
/// whichever resource is currently selected in the focused left pane
/// (`None` while nothing's been navigated to yet) -- pure formatting of
/// fields `fenix_docker` already produces, no `docker inspect` call
/// (see the plan's Scope for why that's deliberately out). `stat`, when
/// the selected resource is a container and a stats tick has landed for
/// it, adds live CPU/MEM lines -- the same live data that used to be
/// crammed into the Containers row now lives here instead, where there's
/// room for it.
pub fn render_details(detail: Option<&DockerDetail>, stat: Option<&ContainerStat>) -> DockerPanel {
    let mut b = Builder::new();
    match detail {
        None => {
            b.push("    Nothing selected", Some(DockerLine { style: DockerLineStyle::Empty, entry: None, dim_from: None, badge: None }));
        }
        Some(DockerDetail::Container(c)) => {
            push_detail_line(&mut b, "ID", &c.id);
            push_detail_line(&mut b, "Name", &c.name);
            push_detail_line(&mut b, "Image", &c.image);
            push_detail_line(&mut b, "State", &c.state);
            push_detail_line(&mut b, "Status", &c.status);
            if let Some(stat) = stat {
                push_detail_line(&mut b, "CPU", &stat.cpu_percent);
                push_detail_line(&mut b, "MEM", &stat.mem_usage);
            }
        }
        Some(DockerDetail::Image(img)) => {
            push_detail_line(&mut b, "ID", &img.id);
            push_detail_line(&mut b, "Repository", &img.repository);
            push_detail_line(&mut b, "Tag", &img.tag);
            push_detail_line(&mut b, "Size", &img.size);
        }
        Some(DockerDetail::Volume(v)) => {
            push_detail_line(&mut b, "Name", &v.name);
            push_detail_line(&mut b, "Driver", &v.driver);
            push_detail_line(&mut b, "Mountpoint", &v.mountpoint);
        }
        Some(DockerDetail::Network(n)) => {
            push_detail_line(&mut b, "ID", &n.id);
            push_detail_line(&mut b, "Name", &n.name);
            push_detail_line(&mut b, "Driver", &n.driver);
            push_detail_line(&mut b, "Scope", &n.scope);
        }
    }
    b.finish()
}

fn push_detail_line(b: &mut Builder, label: &str, value: &str) {
    let prefix = format!("    {label}: ");
    let dim_from = prefix.chars().count();
    b.push(
        &format!("{prefix}{value}"),
        Some(DockerLine { style: DockerLineStyle::Detail, entry: None, dim_from: Some(dim_from), badge: None }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn container(id: &str, name: &str) -> Container {
        Container { id: id.to_string(), name: name.to_string(), image: "nginx:latest".to_string(), status: "Up 3 days".to_string(), state: "running".to_string() }
    }

    fn container_with_state(id: &str, name: &str, state: &str) -> Container {
        Container { state: state.to_string(), ..container(id, name) }
    }

    fn image(id: &str, repo: &str, tag: &str) -> Image {
        Image { id: id.to_string(), repository: repo.to_string(), tag: tag.to_string(), size: "142MB".to_string() }
    }

    fn volume(name: &str) -> Volume {
        Volume { name: name.to_string(), driver: "local".to_string(), mountpoint: format!("/var/lib/docker/volumes/{name}/_data") }
    }

    fn network(id: &str, name: &str) -> Network {
        Network { id: id.to_string(), name: name.to_string(), driver: "bridge".to_string(), scope: "local".to_string() }
    }

    fn stat(container_id: &str) -> ContainerStat {
        ContainerStat { container_id: container_id.to_string(), cpu_percent: "1.23%".to_string(), mem_usage: "10MiB / 2GiB".to_string() }
    }

    #[test]
    fn render_containers_lists_entries_with_no_redundant_header() {
        let panel = render_containers(&[container("id1", "web"), container("id2", "db")]);
        assert!(!panel.text.contains("Containers"));
        let entries: Vec<_> = panel.lines.iter().flatten().filter(|l| l.style == DockerLineStyle::Container).collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry, Some(DockerEntry::Container("id1".to_string())));
    }

    #[test]
    fn render_containers_lines_stay_the_same_length_as_text() {
        let panel = render_containers(&[container("id1", "web")]);
        assert_eq!(panel.text.lines().count(), panel.lines.len());
    }

    #[test]
    fn render_containers_empty_list_shows_a_placeholder() {
        let panel = render_containers(&[]);
        assert!(panel.text.contains("No containers found"));
    }

    #[test]
    fn render_containers_rows_only_show_the_badge_and_name() {
        let panel = render_containers(&[container("id1", "web")]);
        assert!(panel.text.contains("[R] web"));
        assert!(!panel.text.contains("nginx"));
        assert!(!panel.text.contains("Up 3 days"));
    }

    #[test]
    fn render_containers_has_no_footer_hint_line() {
        let panel = render_containers(&[container("id1", "web")]);
        assert!(!panel.text.contains("start"));
        assert!(!panel.text.contains("refresh"));
    }

    #[test]
    fn container_status_badge_covers_every_documented_docker_state() {
        assert_eq!(container_status_badge("running"), ("R", DockerBadgeColor::Good));
        assert_eq!(container_status_badge("paused"), ("P", DockerBadgeColor::Warn));
        assert_eq!(container_status_badge("restarting"), ("~", DockerBadgeColor::Warn));
        assert_eq!(container_status_badge("created"), ("C", DockerBadgeColor::Neutral));
        assert_eq!(container_status_badge("exited"), ("X", DockerBadgeColor::Bad));
        assert_eq!(container_status_badge("dead"), ("D", DockerBadgeColor::Bad));
        assert_eq!(container_status_badge("removing"), ("V", DockerBadgeColor::Bad));
        assert_eq!(container_status_badge("something-unexpected"), ("N", DockerBadgeColor::Neutral));
        assert_eq!(container_status_badge(""), ("N", DockerBadgeColor::Neutral));
    }

    #[test]
    fn render_containers_badge_reflects_each_row_own_state() {
        let panel = render_containers(&[container_with_state("id1", "web", "paused")]);
        let entry = panel.lines[0].as_ref().unwrap();
        assert_eq!(entry.badge.map(|(_, c)| c), Some(DockerBadgeColor::Warn));
        assert!(panel.text.contains("[P] web"));
    }

    #[test]
    fn find_stat_matches_exact_and_prefix_ids() {
        let stats = vec![stat("abc123")];
        assert!(find_stat(&stats, "abc123").is_some());
        assert!(find_stat(&stats, "abc123def456").is_some()); // full id, stat has short prefix
        assert!(find_stat(&stats, "zzz").is_none());
    }

    #[test]
    fn render_images_lists_entries_with_no_redundant_header() {
        let panel = render_images(&[image("id1", "nginx", "latest")]);
        assert!(!panel.text.contains("Images"));
        let entries: Vec<_> = panel.lines.iter().flatten().filter(|l| l.style == DockerLineStyle::Image).collect();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn render_images_empty_list_shows_a_placeholder() {
        let panel = render_images(&[]);
        assert!(panel.text.contains("No images found"));
    }

    #[test]
    fn render_images_with_no_tag_omits_the_trailing_colon() {
        let panel = render_images(&[image("id1", "myimage", "")]);
        assert!(panel.text.contains("myimage  "));
        assert!(!panel.text.contains("myimage:"));
    }

    #[test]
    fn render_volumes_lists_entries_with_the_right_entry() {
        let panel = render_volumes(&[volume("myvol")]);
        let entries: Vec<_> = panel.lines.iter().flatten().filter(|l| l.style == DockerLineStyle::Volume).collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry, Some(DockerEntry::Volume("myvol".to_string())));
    }

    #[test]
    fn render_volumes_empty_list_shows_a_placeholder() {
        let panel = render_volumes(&[]);
        assert!(panel.text.contains("No volumes found"));
    }

    #[test]
    fn render_networks_lists_entries_with_the_right_entry() {
        let panel = render_networks(&[network("id1", "bridge")]);
        let entries: Vec<_> = panel.lines.iter().flatten().filter(|l| l.style == DockerLineStyle::Network).collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry, Some(DockerEntry::Network("id1".to_string())));
    }

    #[test]
    fn render_networks_empty_list_shows_a_placeholder() {
        let panel = render_networks(&[]);
        assert!(panel.text.contains("No networks found"));
    }

    #[test]
    fn render_details_none_shows_nothing_selected() {
        let panel = render_details(None, None);
        assert!(panel.text.contains("Nothing selected"));
    }

    #[test]
    fn render_details_container_shows_its_fields() {
        let panel = render_details(Some(&DockerDetail::Container(container("id1", "web"))), None);
        assert!(panel.text.contains("ID: id1"));
        assert!(panel.text.contains("Name: web"));
        assert!(panel.text.contains("State: running"));
    }

    #[test]
    fn render_details_container_with_a_stat_shows_cpu_and_mem() {
        let panel = render_details(Some(&DockerDetail::Container(container("id1", "web"))), Some(&stat("id1")));
        assert!(panel.text.contains("CPU: 1.23%"));
        assert!(panel.text.contains("MEM: 10MiB / 2GiB"));
    }

    #[test]
    fn render_details_container_without_a_stat_omits_cpu_and_mem() {
        let panel = render_details(Some(&DockerDetail::Container(container("id1", "web"))), None);
        assert!(!panel.text.contains("CPU:"));
        assert!(!panel.text.contains("MEM:"));
    }

    #[test]
    fn render_details_image_shows_its_fields() {
        let panel = render_details(Some(&DockerDetail::Image(image("id1", "nginx", "latest"))), None);
        assert!(panel.text.contains("Repository: nginx"));
        assert!(panel.text.contains("Tag: latest"));
    }

    #[test]
    fn render_details_volume_shows_its_fields() {
        let panel = render_details(Some(&DockerDetail::Volume(volume("myvol"))), None);
        assert!(panel.text.contains("Name: myvol"));
        assert!(panel.text.contains("Driver: local"));
    }

    #[test]
    fn render_details_network_shows_its_fields() {
        let panel = render_details(Some(&DockerDetail::Network(network("id1", "bridge"))), None);
        assert!(panel.text.contains("ID: id1"));
        assert!(panel.text.contains("Name: bridge"));
        assert!(panel.text.contains("Driver: bridge"));
        assert!(panel.text.contains("Scope: local"));
    }

    #[test]
    fn render_details_lines_stay_the_same_length_as_text() {
        let panel = render_details(Some(&DockerDetail::Container(container("id1", "web"))), None);
        assert_eq!(panel.text.lines().count(), panel.lines.len());
    }
}
