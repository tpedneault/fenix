//! The Docker panel (`SPC d`), as a `Feature`: the Lazydocker-style
//! six-pane session, its background stats poller and log follower, its
//! action keys, its remove confirmation and its `x` menu. Everything
//! Docker used to keep on `App` lives on `DockerHost`; `App` reaches it
//! only through the `Feature` hooks and the commands in `SPEC`.

use std::any::Any;

use super::feature::{Feature, FeatureCommand, FeatureEvent, FeatureSpec, Highlights, LeaderEntry};
use super::*;

/// How many trailing lines of `docker logs` the Docker panel's `l`
/// action fetches -- generous enough to actually be useful (most
/// container startup/error output fits well within this) without
/// risking pulling in an unbounded amount of text from a chatty
/// container.
const DOCKER_LOG_TAIL_LINES: usize = 500;

/// Docker's commands and leader keys.
pub(crate) static SPEC: FeatureSpec = FeatureSpec {
    id: "fenix.docker",
    name: "Docker",
    about: "Containers, images and compose projects, and logs",
    commands: &[
        FeatureCommand {
            id: "docker.open",
            description: "Show the Docker container/image panel",
            external: true,
            run: |app| {
                app.with_feature::<DockerHost, _>(|docker, app| docker.open(app));
            },
        },
        FeatureCommand {
            id: "docker.build",
            description: "Build an image from the current project's Dockerfile",
            external: false,
            run: |app| {
                app.with_feature::<DockerHost, _>(|docker, app| docker.build(app));
            },
        },
        FeatureCommand {
            id: "docker.close",
            description: "Close the Docker panel session",
            external: false,
            run: |app| {
                app.with_feature::<DockerHost, _>(|docker, app| docker.close(app));
            },
        },
    ],
    // Reserved entirely for Docker (Lazydocker-style) -- the dashboard
    // used to live at `SPC d d` but moved to `SPC o d` so this whole
    // group is free for docker commands.
    leader: &[
        LeaderEntry::Group(&['d'], "docker"),
        LeaderEntry::Bind(&['d', 'd'], "open docker panel", "docker.open"),
        LeaderEntry::Bind(&['d', 'b'], "build image", "docker.build"),
        LeaderEntry::Bind(&['d', 'q'], "close docker panel", "docker.close"),
    ],
    local_menu: &[],
    local_context: None,
    language_server: None,
    tasks: None,
    hover_fallback: None,
    completion_at_cursor: None,
    completion_pool_at: None,
    hover_first: None,
    picker_confirm: None,
    picker_cancel: None,
    modeline_clock: None,
    page_action: None,
    page_context: None,
    page_title: None,
    page_after_layout: None,
    on_event: None,
    settings: &[],
    services: &[],
    contributes: &[],
    session_documents: None,
    session_reopen: None,
    home: None,
    home_activate: None,
    doctor: None,
    opener: None,
};

/// What Docker's background threads report, as a `FeatureEvent`'s
/// payload.
#[derive(Debug)]
enum DockerEvent {
    /// A fresh `docker stats --no-stream` snapshot, from the active
    /// session's background poller.
    Stats(Vec<fenix_docker::ContainerStat>),
    /// One new line of `docker logs -f` output for the session's Logs
    /// buffer. Carries the buffer explicitly, so a line from a follower
    /// torn down a moment ago can be told apart and dropped.
    LogLine(BufferId, String),
    /// The log follower's `docker logs -f` child exited on its own.
    LogEnded(BufferId),
}

impl DockerEvent {
    /// The hang log's name for it -- the names these had as
    /// `FenixUserEvent` variants.
    fn label(&self) -> &'static str {
        match self {
            Self::Stats(_) => "StatsReady",
            Self::LogLine(..) => "LogLine",
            Self::LogEnded(_) => "LogEnded",
        }
    }

    fn into_user_event(self) -> FenixUserEvent {
        FenixUserEvent::Feature(FeatureEvent {
            feature: SPEC.id,
            label: self.label(),
            payload: Box::new(self),
        })
    }
}

/// Which of a `DockerSession`'s six panes is currently focused, if
/// any -- what the action-key routing and `sync_details` key off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DockerPaneRole {
    Containers,
    Images,
    Volumes,
    Networks,
    Status,
    Logs,
}

/// One active Lazydocker-style multi-pane session (`SPC d d`) --
/// `DockerHost::session` is `None` when no such session is open. A
/// singleton, not one-per-workspace: re-pressing `SPC d d` while one
/// already exists refocuses/refreshes it in place instead of opening a
/// second (see `DockerHost::open`'s own doc comment) -- this session
/// owns live background threads/processes that a second, orphaned
/// session would leak.
///
/// Six panes, not five: Containers/Images/Volumes/Networks stacked on
/// the left, then Status (small, on top) and Logs (below it) stacked on
/// the right -- Status always shows plain info fields (+ live CPU/MEM
/// once a stats tick has landed, if the selection is a container) for
/// whatever's under the cursor in the focused left pane; Logs is
/// dedicated purely to `l`'s live-streamed log output. Splitting these
/// into two buffers (rather than the original single "Details" pane
/// toggling between the two) is what let `sync_details` drop its old
/// "skip while following logs" gate -- moving the cursor in Containers
/// now always refreshes Status, streaming logs or not, since they're no
/// longer the same buffer.
///
/// Role (which pane is which) lives here, not as extra `BufferKind`
/// variants: all six buffers stay tagged the existing `BufferKind::
/// Docker` -- the action-key routing and `sync_details` both key off
/// *which pane is focused* (`DockerPaneRole`/`focused_role`) instead.
pub(super) struct DockerSession {
    pub(super) workspace_index: usize,
    pub(super) containers_pane: fenix_window::WindowId,
    pub(super) images_pane: fenix_window::WindowId,
    pub(super) volumes_pane: fenix_window::WindowId,
    pub(super) networks_pane: fenix_window::WindowId,
    pub(super) status_pane: fenix_window::WindowId,
    pub(super) logs_pane: fenix_window::WindowId,
    pub(super) containers_buffer: BufferId,
    pub(super) images_buffer: BufferId,
    pub(super) volumes_buffer: BufferId,
    pub(super) networks_buffer: BufferId,
    pub(super) status_buffer: BufferId,
    pub(super) logs_buffer: BufferId,
    /// Last-listed containers/images/volumes/networks, cached so
    /// `sync_details` and the stats poller can re-render merged rows
    /// without re-running `docker ps`/`docker images`/`docker volume
    /// ls`/`docker network ls` on every cursor move/tick.
    pub(super) containers: Vec<fenix_docker::Container>,
    pub(super) images: Vec<fenix_docker::Image>,
    pub(super) volumes: Vec<fenix_docker::Volume>,
    pub(super) networks: Vec<fenix_docker::Network>,
    /// The most recent `docker stats --no-stream` snapshot, cached so
    /// the Status pane can show live CPU/MEM for whichever container is
    /// currently selected without needing its own separate fetch --
    /// same "cache what the poller already produced" reasoning as
    /// `containers`/`images`/`volumes`.
    pub(super) last_stats: Vec<fenix_docker::ContainerStat>,
    /// `Some(id)` once the Logs pane has been switched into live
    /// log-follow mode for container `id`; `None` before `l` has ever
    /// been pressed this session.
    pub(super) logs_container: Option<String>,
    /// The session's background stats poller -- `None` when the session
    /// was opened without a real `event_proxy` (every test, since `App::
    /// with_file` never sets one; see `App::new`'s own doc comment), in
    /// which case there's nothing to wake and nothing spawns. Never read
    /// again once set -- it's a pure RAII guard, held only so its `Drop`
    /// impl runs (stopping the thread) when the session closes/is
    /// replaced, the same "hold it just for the side effect of dropping
    /// it" idiom as a `MutexGuard`/`TempDir`.
    #[allow(dead_code)]
    stats_poller: Option<DockerStatsPoller>,
    /// The session's live log-follower, if `logs_container` names a
    /// container currently being tailed -- same "held only for its
    /// `Drop` side effect" RAII shape as `stats_poller`.
    #[allow(dead_code)]
    pub(super) log_follower: Option<DockerLogFollower>,
}

/// A background `docker stats --no-stream` poller for one open Docker
/// session -- ticks every `STATS_POLL_INTERVAL`, each tick sending a
/// fresh `DockerEvent::Stats`. Never re-lists containers itself (see
/// `DockerSession::containers`'s own doc comment) -- purely a stats
/// source, merged against whatever the session already has cached by
/// `DockerHost::apply_stats`.
///
/// `Drop` is what makes teardown *real*, not just a "bounded leftover"
/// the way an inert `HashMap` entry would be (see `DockerSession`'s own
/// doc comment on why role lives there): whenever a `DockerSession` is
/// replaced or dropped, its poller's `stop` flag is set and its thread
/// is joined before the struct finishes dropping, so it's guaranteed
/// gone, not just abandoned to keep running in the background.
struct DockerStatsPoller {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl DockerStatsPoller {
    /// Takes a plain `send` callback (`true` = delivered, `false` =
    /// the receiving end is gone, stop) rather than an `EventLoopProxy`
    /// directly -- the real call site (`DockerHost::open`) wraps
    /// `proxy.send_event(...).is_ok()` into one, but this indirection
    /// is what makes the thread-lifecycle logic itself (spawn, tick,
    /// stop-check, join-on-drop) directly unit-testable with a plain
    /// closure, without needing a real winit `EventLoop` (which nothing
    /// outside a running GUI can construct headlessly). `fetch` is the
    /// stats source (`fenix_docker::container_stats` for real) for the
    /// same reason: with an engine running, one `docker stats` takes
    /// seconds, which a test of the thread's lifecycle shouldn't wait on.
    fn spawn(
        fetch: impl Fn() -> Vec<fenix_docker::ContainerStat> + Send + 'static,
        send: impl Fn(DockerEvent) -> bool + Send + 'static,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let handle = std::thread::spawn(move || loop {
            if thread_stop.load(Ordering::Relaxed) {
                return;
            }
            let stats = fetch();
            // `false` here means the receiving end is gone (the whole
            // app is shutting down) -- nothing left to wake, so the
            // thread can just end rather than looping forever sending
            // into the void.
            if !send(DockerEvent::Stats(stats)) {
                return;
            }
            let mut waited = Duration::ZERO;
            while waited < STATS_POLL_INTERVAL {
                if thread_stop.load(Ordering::Relaxed) {
                    return;
                }
                let step = STATS_POLL_STOP_CHECK.min(STATS_POLL_INTERVAL - waited);
                std::thread::sleep(step);
                waited += step;
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for DockerStatsPoller {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// One live `docker logs -f` child + the thread reading its stdout, for
/// whichever container the Docker session's Logs pane is currently
/// following. Exactly one exists per session at a time -- pressing `l`
/// on a different (or the same) row always replaces the session's
/// `log_follower` field first, and dropping the old value's `Drop` impl
/// (below) tears it down before the new one is spawned, so there's
/// never two live `docker logs -f` children per session.
pub(super) struct DockerLogFollower {
    child: std::process::Child,
    stop: Arc<AtomicBool>,
    reader_handle: Option<JoinHandle<()>>,
}

impl DockerLogFollower {
    /// `send` mirrors `DockerStatsPoller::spawn`'s own callback shape
    /// (real callers wrap `proxy.send_event(...).is_ok()`), for the
    /// same "testable without a real winit `EventLoop`" reason.
    fn spawn(
        mut child: std::process::Child,
        buffer_id: BufferId,
        send: impl Fn(DockerEvent) -> bool + Send + 'static,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let reader_handle = child.stdout.take().map(|stdout| {
            std::thread::spawn(move || {
                use std::io::BufRead;
                for line in std::io::BufReader::new(stdout).lines() {
                    if thread_stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(line) = line else { break };
                    if !send(DockerEvent::LogLine(buffer_id, line)) {
                        return;
                    }
                }
                // Reached on real EOF (the process exited on its own,
                // e.g. the container stopped) -- a deliberate `kill()`
                // in `Drop` also produces EOF here, which is exactly
                // why the `stop` check above (not this one) is what
                // distinguishes "torn down on purpose, stay quiet" from
                // "actually ended, tell Logs."
                if !thread_stop.load(Ordering::Relaxed) {
                    let _ = send(DockerEvent::LogEnded(buffer_id));
                }
            })
        });
        Self {
            child,
            stop,
            reader_handle,
        }
    }
}

impl Drop for DockerLogFollower {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Kills `docker logs -f`, which closes its stdout pipe from the
        // writing end -- that's what unblocks the reader thread's
        // blocking `BufRead::lines()` call with an EOF/error instead of
        // it hanging forever waiting for more output that will never
        // come.
        let _ = self.child.kill();
        let _ = self.child.wait(); // reap, avoid a zombie process
        if let Some(handle) = self.reader_handle.take() {
            let _ = handle.join();
        }
    }
}

/// A `send` callback for the background threads, posting through the
/// event loop.
fn proxy_sender(
    proxy: winit::event_loop::EventLoopProxy<FenixUserEvent>,
) -> impl Fn(DockerEvent) -> bool + Send + 'static {
    move |event| proxy.send_event(event.into_user_event()).is_ok()
}

/// Everything the Docker panel keeps between frames.
#[derive(Default)]
pub(crate) struct DockerHost {
    /// Per-line metadata for every real `BufferKind::Docker` buffer
    /// currently open (`SPC d d`), keyed by `BufferId` -- same per-buffer-
    /// keyed precedent as `dashboard_lines`/`dired_lines`. Consulted by
    /// the action keys (`s`/`S`/`R`/`r`/`d`) to know what the cursor's
    /// current line targets, and by the panel's own syntax-highlight
    /// coloring.
    pub(super) lines: HashMap<BufferId, Vec<Option<docker_panel::DockerLine>>>,
    /// Armed by `d` on a Docker buffer row until the next keypress
    /// confirms (`y`) or cancels (anything else) -- same "wait for
    /// exactly one more raw key" shape as `fenix-vim`'s own `r<char>`,
    /// applied here to a destructive remove instead of a replace.
    /// Overrides the modeline the same way `Mode::Command`/`Mode::Search`
    /// already do (see `modeline_pieces`/`confirm_text`) to show what's
    /// being confirmed.
    pub(super) confirm_remove: Option<docker_panel::DockerEntry>,
    /// Set by `x` on a Docker pane -- shows a which-key-style popup
    /// listing that pane's available action keys (real Lazydocker's own
    /// "view command options" convention). Purely informational, not a
    /// capturing prompt like `confirm_remove`/`explorer_prompt`: the
    /// `key_arrived` hook clears it unconditionally on every keypress
    /// *before* dispatching that keypress normally, so pressing any key
    /// dismisses the popup and still does whatever that key would
    /// otherwise do.
    pub(super) menu_open: bool,
    /// The active Docker multi-pane session (`SPC d d`), if any -- see
    /// `DockerSession`'s own doc comment.
    pub(super) session: Option<DockerSession>,
}

impl DockerHost {
    /// `SPC d d`: opens (or, if one's already open, refocuses/refreshes)
    /// the Lazydocker-style multi-pane session -- Containers/Images/
    /// Volumes stacked on the left, Status (small) and Logs stacked on
    /// the right, each with its own title bar (`pane_titles`).
    /// Deliberately breaks from the "always a new buffer/pane, no
    /// dedup" precedent `open_dashboard`/this feature's own earlier
    /// single-buffer form used: a session now owns live background
    /// threads/processes, and a second, orphaned one would leak them.
    /// Gets its own workspace (`SPC TAB`) so it doesn't disturb whatever
    /// splits/buffers were already open.
    pub(super) fn open(&mut self, app: &mut App) {
        if let Some(session) = &self.session {
            let (workspace_index, containers_pane) =
                (session.workspace_index, session.containers_pane);
            app.workspaces.switch_to_index(workspace_index);
            app.windows_mut().focus(containers_pane);
            self.refresh(app);
            app.wake_caret();
            return;
        }

        let containers = fenix_docker::list_containers();
        let images = fenix_docker::list_images();
        let volumes = fenix_docker::list_volumes();
        let networks = fenix_docker::list_networks();

        let containers_panel = docker_panel::render_containers(&containers);
        let images_panel = docker_panel::render_images(&images);
        let volumes_panel = docker_panel::render_volumes(&volumes);
        let networks_panel = docker_panel::render_networks(&networks);
        let status_panel = docker_panel::render_details(None, None);

        let containers_buffer = app.buffers.open_docker(&containers_panel.text);
        let images_buffer = app.buffers.open_docker(&images_panel.text);
        let volumes_buffer = app.buffers.open_docker(&volumes_panel.text);
        let networks_buffer = app.buffers.open_docker(&networks_panel.text);
        let status_buffer = app.buffers.open_docker(&status_panel.text);
        let logs_buffer = app.buffers.open_docker("");
        self.lines.insert(containers_buffer, containers_panel.lines);
        self.lines.insert(images_buffer, images_panel.lines);
        self.lines.insert(volumes_buffer, volumes_panel.lines);
        self.lines.insert(networks_buffer, networks_panel.lines);
        self.lines.insert(status_buffer, status_panel.lines);

        let cursor = Cursor::at_start();
        app.workspaces.new_workspace(containers_buffer, cursor);
        let workspace_index = app.workspaces.active_index();
        let containers_pane = app.focused_pane_id();

        // Right column first, at the outer split -- default 0.5 ratio,
        // narrowed to ~35/65 next (left column is 3 stacked panes, so
        // it needs less width per pane than the right column's 2).
        let status_pane = app.windows_mut().split(SplitKind::Vertical, status_buffer);
        app.workspaces
            .active_pane_states_mut()
            .insert(status_pane, PaneState::seeded_at(cursor));
        app.windows_mut().resize_focused(-0.15);

        // Right column itself: Status (small, on top) / Logs (the rest)
        // -- shrink Status down from the default 50/50 split.
        let logs_pane = app.windows_mut().split(SplitKind::Horizontal, logs_buffer);
        app.workspaces
            .active_pane_states_mut()
            .insert(logs_pane, PaneState::seeded_at(cursor));
        app.windows_mut().resize_focused(-0.2);

        // Left column: Containers/Images/Volumes/Networks stacked.
        // Containers keeps its deliberate 50% (see the layout test's own
        // name); Images/Volumes/Networks split the other 50% into equal
        // thirds (1/6 of the column each). Three plain 0.5/0.5 splits
        // would only produce that for *three* panes (50/25/25) -- for
        // four, the Images split needs shrinking from 1/4 to 1/6 of the
        // column (1/3, not 1/2, of the remaining half) so the Volumes/
        // Networks split after it can still land on an even half each.
        app.windows_mut().focus(containers_pane);
        let images_pane = app
            .windows_mut()
            .split(SplitKind::Horizontal, images_buffer);
        app.workspaces
            .active_pane_states_mut()
            .insert(images_pane, PaneState::seeded_at(cursor));
        let volumes_pane = app
            .windows_mut()
            .split(SplitKind::Horizontal, volumes_buffer);
        app.workspaces
            .active_pane_states_mut()
            .insert(volumes_pane, PaneState::seeded_at(cursor));
        app.windows_mut().resize_focused(-1.0 / 6.0);
        let networks_pane = app
            .windows_mut()
            .split(SplitKind::Horizontal, networks_buffer);
        app.workspaces
            .active_pane_states_mut()
            .insert(networks_pane, PaneState::seeded_at(cursor));
        app.windows_mut().focus(containers_pane);

        app.pane_titles
            .insert(containers_pane, "1. Containers".to_string());
        app.pane_titles.insert(images_pane, "2. Images".to_string());
        app.pane_titles
            .insert(volumes_pane, "3. Volumes".to_string());
        app.pane_titles
            .insert(networks_pane, "4. Networks".to_string());
        app.pane_titles.insert(status_pane, "5. Status".to_string());
        app.pane_titles.insert(logs_pane, "6. Logs".to_string());

        // Only spawned for a real, `main.rs`-launched `App` -- every
        // test's `event_proxy` stays `None` (see `App::new`'s own doc
        // comment), so a session opened in a test just never ticks,
        // which is fine (nothing there needs a working `docker` either).
        let stats_poller = app.event_proxy.clone().map(|proxy| {
            DockerStatsPoller::spawn(fenix_docker::container_stats, proxy_sender(proxy))
        });

        self.session = Some(DockerSession {
            workspace_index,
            containers_pane,
            images_pane,
            volumes_pane,
            networks_pane,
            status_pane,
            logs_pane,
            containers_buffer,
            images_buffer,
            volumes_buffer,
            networks_buffer,
            status_buffer,
            logs_buffer,
            containers,
            images,
            volumes,
            networks,
            last_stats: Vec::new(),
            logs_container: None,
            stats_poller,
            log_follower: None,
        });

        app.refresh_project_root();
        app.wake_caret();
    }

    /// Rewrites `id`'s buffer text from a freshly-rendered `DockerPanel`,
    /// resetting every pane currently showing it back to the top -- same
    /// "rewrite the whole affected span as one step" tool (`Buffer::
    /// replace_range`) `set_dired_state` already uses for the dired
    /// buffer, and same "a refreshed listing can reorder/add/remove
    /// rows, so an old cursor position is meaningless against it"
    /// reasoning. Used by the manual `u` refresh and by switching
    /// Details between info/log content -- deliberately *not* used by
    /// the automatic stats-tick refresh (Phase 5), which needs to
    /// preserve the user's cursor instead of resetting it every ~2s.
    pub(super) fn set_buffer(
        &mut self,
        app: &mut App,
        id: BufferId,
        panel: docker_panel::DockerPanel,
    ) {
        self.lines.insert(id, panel.lines);
        if let Some(ob) = app.buffers.get_mut(id) {
            let end = ob.buffer.len_chars();
            let mut scratch_cursor = Cursor::at_start();
            ob.buffer
                .replace_range(&mut scratch_cursor, 0, end, &panel.text);
        }
        for pane in app.windows().windows() {
            if app.windows().content(pane) == Some(&id) {
                let ps = app.pane_state_mut(pane);
                *ps = PaneState::seeded_at(Cursor::at_start());
            }
        }
    }

    /// `DockerEvent::Stats` handling: caches the fresh `docker stats`
    /// snapshot and re-syncs Status so a container's live CPU/MEM
    /// updates in place if one is currently selected -- Containers rows
    /// no longer show stats inline (see `docker_panel::render_
    /// containers`'s own doc comment for why), so there's no Containers-
    /// buffer rewrite here at all anymore, and Status is small/read-only
    /// enough that resetting it to the top on every tick (`sync_
    /// details`'s own `set_buffer` reset-to-top behavior) is harmless.
    /// A no-op if no session is open (the stats poller is always torn
    /// down with its session, but a stray already-in-flight event
    /// landing right at teardown is a cheap no-op to discard).
    pub(super) fn apply_stats(&mut self, app: &mut App, stats: Vec<fenix_docker::ContainerStat>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.last_stats = stats;
        self.sync_details(app);
    }

    /// `u` (on any of the four left panes, Status, or Logs): re-lists
    /// containers/images/volumes/networks from `docker` and re-renders
    /// all four, then re-syncs Status from whatever's now under the
    /// cursor.
    pub(super) fn refresh(&mut self, app: &mut App) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let (containers_buffer, images_buffer, volumes_buffer, networks_buffer) = (
            session.containers_buffer,
            session.images_buffer,
            session.volumes_buffer,
            session.networks_buffer,
        );

        let containers = fenix_docker::list_containers();
        let images = fenix_docker::list_images();
        let volumes = fenix_docker::list_volumes();
        let networks = fenix_docker::list_networks();
        let containers_panel = docker_panel::render_containers(&containers);
        let images_panel = docker_panel::render_images(&images);
        let volumes_panel = docker_panel::render_volumes(&volumes);
        let networks_panel = docker_panel::render_networks(&networks);

        if let Some(session) = self.session.as_mut() {
            session.containers = containers;
            session.images = images;
            session.volumes = volumes;
            session.networks = networks;
        }
        self.set_buffer(app, containers_buffer, containers_panel);
        self.set_buffer(app, images_buffer, images_panel);
        self.set_buffer(app, volumes_buffer, volumes_panel);
        self.set_buffer(app, networks_buffer, networks_panel);
        self.sync_details(app);
    }

    /// Which of the current session's six panes (if any) is focused
    /// right now -- `None` when there's no session, the active
    /// *workspace* isn't even the session's own one, or the focused
    /// pane belongs to something else entirely. The workspace check
    /// matters because `WindowId`s are only unique *within* one
    /// `WindowTree` (each workspace's tree keeps its own id counter
    /// starting from 0) -- comparing pane ids without first confirming
    /// the session's workspace is the active one could false-positive
    /// against a numerically-identical pane id in a completely
    /// different, unrelated workspace.
    pub(super) fn focused_role(&self, app: &App) -> Option<DockerPaneRole> {
        let session = self.session.as_ref()?;
        if app.workspaces.active_index() != session.workspace_index {
            return None;
        }
        let focused = app.focused_pane_id();
        if focused == session.containers_pane {
            Some(DockerPaneRole::Containers)
        } else if focused == session.images_pane {
            Some(DockerPaneRole::Images)
        } else if focused == session.volumes_pane {
            Some(DockerPaneRole::Volumes)
        } else if focused == session.networks_pane {
            Some(DockerPaneRole::Networks)
        } else if focused == session.status_pane {
            Some(DockerPaneRole::Status)
        } else if focused == session.logs_pane {
            Some(DockerPaneRole::Logs)
        } else {
            None
        }
    }

    /// The pane whose title bar is numbered `n` (`1. Containers` etc.,
    /// see `open`'s own title strings) -- backs the digit-key
    /// jump-to-pane shortcut. `None` outside a session or for a number
    /// with no matching pane.
    pub(super) fn pane_by_number(&self, n: u32) -> Option<fenix_window::WindowId> {
        let session = self.session.as_ref()?;
        match n {
            1 => Some(session.containers_pane),
            2 => Some(session.images_pane),
            3 => Some(session.volumes_pane),
            4 => Some(session.networks_pane),
            5 => Some(session.status_pane),
            6 => Some(session.logs_pane),
            _ => None,
        }
    }

    /// What the cursor's current line on a Docker buffer targets -- a
    /// no-op (`None`) for a header/footer/blank line, mirroring
    /// `dashboard_activate_selected`'s own lookup shape. Reads whichever
    /// buffer is *focused*, which is always the right one here since
    /// every call site is only reached after routing has already
    /// confirmed the focused pane is a specific session role.
    pub(super) fn entry_at_cursor(&self, app: &App) -> Option<docker_panel::DockerEntry> {
        let cursor = app.cursor();
        let line = app.open().buffer.line_col(&cursor).0;
        self.lines
            .get(&app.focused_buffer_id())
            .and_then(|lines| lines.get(line))
            .and_then(|meta| meta.as_ref())
            .and_then(|meta| meta.entry.clone())
    }

    /// Re-renders the Status pane from whichever row is now under the
    /// cursor in the focused left pane, looked up in the session's own
    /// cached `containers`/`images`/`volumes` (plus `last_stats`, for a
    /// container's live CPU/MEM) -- pure formatting, no `docker` call.
    /// A no-op if nothing's selected or there's no session at all.
    /// Unconditional now that Status and Logs are separate buffers (see
    /// `DockerSession`'s own doc comment) -- moving the cursor around
    /// while logs are streaming no longer needs to skip this the way
    /// the single shared "Details" pane once did.
    pub(super) fn sync_details(&mut self, app: &mut App) {
        let Some(entry) = self.entry_at_cursor(app) else {
            return;
        };
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let (detail, stat) = match entry {
            docker_panel::DockerEntry::Container(id) => {
                let stat = docker_panel::find_stat(&session.last_stats, &id).cloned();
                (
                    session
                        .containers
                        .iter()
                        .find(|c| c.id == id)
                        .cloned()
                        .map(docker_panel::DockerDetail::Container),
                    stat,
                )
            }
            docker_panel::DockerEntry::Image(id) => (
                session
                    .images
                    .iter()
                    .find(|i| i.id == id)
                    .cloned()
                    .map(docker_panel::DockerDetail::Image),
                None,
            ),
            docker_panel::DockerEntry::Volume(name) => (
                session
                    .volumes
                    .iter()
                    .find(|v| v.name == name)
                    .cloned()
                    .map(docker_panel::DockerDetail::Volume),
                None,
            ),
            docker_panel::DockerEntry::Network(id) => (
                session
                    .networks
                    .iter()
                    .find(|n| n.id == id)
                    .cloned()
                    .map(docker_panel::DockerDetail::Network),
                None,
            ),
        };
        let status_buffer = session.status_buffer;
        self.set_buffer(
            app,
            status_buffer,
            docker_panel::render_details(detail.as_ref(), stat.as_ref()),
        );
    }

    /// `s` on the Containers pane: starts the container under the cursor.
    fn start_selected(&mut self, app: &mut App) {
        if let Some(docker_panel::DockerEntry::Container(id)) = self.entry_at_cursor(app) {
            if let Err(err) = fenix_docker::start_container(&id) {
                app.set_error(format!("docker start failed: {err}"));
            }
            self.refresh(app);
        }
    }

    /// `S` on the Containers pane: stops the container under the cursor.
    fn stop_selected(&mut self, app: &mut App) {
        if let Some(docker_panel::DockerEntry::Container(id)) = self.entry_at_cursor(app) {
            if let Err(err) = fenix_docker::stop_container(&id) {
                app.set_error(format!("docker stop failed: {err}"));
            }
            self.refresh(app);
        }
    }

    /// `R` on the Containers pane: restarts the container under the
    /// cursor.
    fn restart_selected(&mut self, app: &mut App) {
        if let Some(docker_panel::DockerEntry::Container(id)) = self.entry_at_cursor(app) {
            if let Err(err) = fenix_docker::restart_container(&id) {
                app.set_error(format!("docker restart failed: {err}"));
            }
            self.refresh(app);
        }
    }

    /// `r` on the Images pane: creates and starts a detached container
    /// from the image under the cursor.
    fn run_selected(&mut self, app: &mut App) {
        if let Some(docker_panel::DockerEntry::Image(id)) = self.entry_at_cursor(app) {
            if let Err(err) = fenix_docker::run_image(&id) {
                app.set_error(format!("docker run failed: {err}"));
            }
            self.refresh(app);
        }
    }

    /// `l` on the Containers pane: switches the Logs pane into live
    /// log-follow mode for the container under the cursor -- spawns
    /// `docker logs -f`, replacing any previous follower first (so
    /// there's never more than one live child per session; see
    /// `DockerLogFollower`'s own doc comment), then forwards each new
    /// line into Logs as it arrives via `DockerEvent::LogLine` (see
    /// `append_log_line`). Doesn't move focus -- Containers stays
    /// focused, Logs just starts streaming in place.
    ///
    /// Without a real `event_proxy` (every test, and any launch that
    /// somehow reaches this before the event loop exists) there's
    /// nothing that could ever deliver a `LogLine` event, so this falls
    /// back to `container_logs`' one-shot snapshot instead -- the
    /// feature still does something sensible rather than silently
    /// showing nothing.
    pub(super) fn view_logs_selected(&mut self, app: &mut App) {
        let Some(docker_panel::DockerEntry::Container(id)) = self.entry_at_cursor(app) else {
            return;
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let logs_buffer = session.logs_buffer;
        session.log_follower = None; // tear down any previous follower first

        let Some(proxy) = app.event_proxy.clone() else {
            let text = match fenix_docker::container_logs(&id, DOCKER_LOG_TAIL_LINES) {
                Ok(text) if text.is_empty() => "(no log output)\n".to_string(),
                Ok(text) => text,
                Err(err) => format!("fenix: couldn't fetch logs for {id}: {err}\n"),
            };
            self.set_buffer(
                app,
                logs_buffer,
                docker_panel::DockerPanel {
                    text,
                    lines: Vec::new(),
                },
            );
            return;
        };

        let Some(child) = fenix_docker::spawn_log_follower(&id, DOCKER_LOG_TAIL_LINES) else {
            let text = format!("fenix: couldn't start `docker logs -f` for {id}\n");
            self.set_buffer(
                app,
                logs_buffer,
                docker_panel::DockerPanel {
                    text,
                    lines: Vec::new(),
                },
            );
            return;
        };

        let follower = DockerLogFollower::spawn(child, logs_buffer, proxy_sender(proxy));
        if let Some(session) = self.session.as_mut() {
            session.log_follower = Some(follower);
            session.logs_container = Some(id.clone());
        }
        let text = format!("-- following logs for {id} --\n");
        self.set_buffer(
            app,
            logs_buffer,
            docker_panel::DockerPanel {
                text,
                lines: Vec::new(),
            },
        );
    }

    /// `DockerEvent::LogLine` handling: appends one new line to the
    /// Logs buffer. Discards a stray event for a buffer that isn't (or
    /// isn't anymore) the current session's Logs buffer -- a cheap
    /// no-op, not worth racing to prevent at teardown.
    ///
    /// Auto-follow: for every pane currently showing this buffer, if
    /// its cursor was already on the last line *before* this line
    /// arrived, it's moved to the new last line after appending too, so
    /// the view stays pinned to the bottom -- the disclosed "simple
    /// heuristic" from the plan. A pane whose cursor was anywhere else
    /// (the user scrolled up to read earlier output) is left alone;
    /// navigating back to the end (real Vim `G`) re-arms auto-follow
    /// for it on the next line, since the same "already at the end"
    /// check will then be true again.
    pub(super) fn append_log_line(&mut self, app: &mut App, buffer_id: BufferId, line: String) {
        if self.session.as_ref().map(|s| s.logs_buffer) != Some(buffer_id) {
            return;
        }
        let panes_at_end: Vec<fenix_window::WindowId> = app
            .windows()
            .windows()
            .into_iter()
            .filter(|&pane| app.windows().content(pane) == Some(&buffer_id))
            .filter(|&pane| {
                let Some(ob) = app.buffers.get(buffer_id) else {
                    return false;
                };
                let (row, _) = ob.buffer.line_col(&app.pane_state(pane).cursor);
                row + 1 >= ob.buffer.visual_line_count()
            })
            .collect();

        let Some(ob) = app.buffers.get_mut(buffer_id) else {
            return;
        };
        let mut append_cursor = Cursor {
            char_idx: ob.buffer.len_chars(),
            sticky_col: 0,
        };
        ob.buffer
            .insert_str(&mut append_cursor, &format!("{line}\n"));

        if let Some(ob) = app.buffers.get(buffer_id) {
            let last_line = ob.buffer.visual_line_count().saturating_sub(1);
            let end_char = ob.buffer.line_start_char(last_line);
            for pane in panes_at_end {
                app.pane_state_mut(pane).cursor = Cursor {
                    char_idx: end_char,
                    sticky_col: 0,
                };
            }
        }
    }

    /// `DockerEvent::LogEnded` handling: appends a marker line and
    /// drops the session's `log_follower` (the child already exited on
    /// its own in this path -- `Drop` still runs for consistency, but
    /// `kill()`/`wait()` on an already-gone process are harmless no-ops).
    pub(super) fn finish_log_stream(&mut self, app: &mut App, buffer_id: BufferId) {
        if self.session.as_ref().map(|s| s.logs_buffer) != Some(buffer_id) {
            return;
        }
        if let Some(ob) = app.buffers.get_mut(buffer_id) {
            let mut append_cursor = Cursor {
                char_idx: ob.buffer.len_chars(),
                sticky_col: 0,
            };
            ob.buffer
                .insert_str(&mut append_cursor, "-- log stream ended --\n");
        }
        if let Some(session) = self.session.as_mut() {
            session.log_follower = None;
        }
    }

    /// `SPC d b`: builds an image from the current project root's
    /// `Dockerfile` (falling back to the process's cwd with no detected
    /// project) -- doesn't need a particular pane focused, unlike the
    /// per-row actions above, since a build targets the project, not a
    /// selected container/image. Refreshes an already-open session so a
    /// freshly built image shows up without a separate manual `u`.
    fn build(&mut self, app: &mut App) {
        let context_dir = app
            .project_root
            .clone()
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let tag = context_dir
            .file_name()
            .map(|n| format!("{}:latest", n.to_string_lossy()));
        match fenix_docker::build_image(&context_dir, tag.as_deref()) {
            Ok(_) => app.set_message(format!(
                "docker build succeeded for {}",
                context_dir.display()
            )),
            Err(err) => app.set_error(format!("docker build failed: {err}")),
        }
        if self.session.is_some() {
            self.refresh(app);
        }
        app.wake_caret();
    }

    /// `SPC d q`, or `SPC b k` on any of the session's buffers (see the
    /// `close_buffer` hook): tears down the whole multi-pane session --
    /// closes all six buffers, clears their `lines`/`pane_titles`
    /// entries, and removes the session's own workspace (switched to
    /// first, so this is correct regardless of whichever workspace
    /// happens to be active when it's called -- always succeeds, since
    /// `open` only ever creates a *new* workspace for a session, never
    /// reuses the one that was active before it, so there's always at
    /// least one other workspace left).
    pub(super) fn close(&mut self, app: &mut App) {
        let Some(session) = self.session.take() else {
            return;
        };
        for id in [
            session.containers_buffer,
            session.images_buffer,
            session.volumes_buffer,
            session.networks_buffer,
            session.status_buffer,
            session.logs_buffer,
        ] {
            app.buffers.close(id);
            self.lines.remove(&id);
        }
        for pane in [
            session.containers_pane,
            session.images_pane,
            session.volumes_pane,
            session.networks_pane,
            session.status_pane,
            session.logs_pane,
        ] {
            app.pane_titles.remove(&pane);
        }
        app.workspaces.switch_to_index(session.workspace_index);
        app.workspaces.remove_active();
        app.refresh_project_root();
        app.wake_caret();
    }

    /// Modeline text while a Docker remove is armed (`d` was just
    /// pressed) -- same "override the modeline with raw prompt text"
    /// mechanism `Mode::Command`/`Mode::Search` already use.
    pub(super) fn confirm_text(&self) -> Option<String> {
        let entry = self.confirm_remove.as_ref()?;
        let what = match entry {
            docker_panel::DockerEntry::Container(_) => "container",
            docker_panel::DockerEntry::Image(_) => "image",
            // Volumes are list-only for now (see the multi-pane plan's
            // Scope: no `fenix_docker` volume-remove function exists
            // yet) -- unreachable today since nothing arms `d` on a
            // Volume entry, kept only for match exhaustiveness.
            docker_panel::DockerEntry::Volume(_) => "volume",
            docker_panel::DockerEntry::Network(_) => "network",
        };
        Some(format!("Remove this {what}? (y/n)"))
    }

    /// Resolves an armed Docker remove: `y` confirms and removes,
    /// anything else cancels. Either way clears `confirm_remove` and
    /// returns the modeline to normal.
    pub(super) fn confirm_key(&mut self, app: &mut App, keypress: KeyPress) {
        let entry = self.confirm_remove.take();
        if keypress.code == KeyCode::Char('y') {
            if let Some(entry) = entry {
                let result = match entry {
                    docker_panel::DockerEntry::Container(id) => fenix_docker::remove_container(&id),
                    docker_panel::DockerEntry::Image(id) => fenix_docker::remove_image(&id),
                    // Unreachable today -- see `confirm_text`'s
                    // matching comment.
                    docker_panel::DockerEntry::Volume(_) => Ok(String::new()),
                    docker_panel::DockerEntry::Network(id) => fenix_docker::remove_network(&id),
                };
                if let Err(err) = result {
                    app.set_error(format!("docker remove failed: {err}"));
                }
                self.refresh(app);
            }
        }
        app.wake_caret();
    }

    /// Lazydocker-style multi-pane panel: ordinary motions (`hjkl`,
    /// `gg`/`G`, `/` search) reach Vim unchanged in every one of the
    /// panes -- only a small, pane-scoped action-key set is claimed
    /// here first, same "claim a few keys, everything else falls
    /// through" shape the dired buffer already uses. Routed by *which
    /// pane is focused*, not by buffer kind alone -- all six panes share
    /// `BufferKind::Docker` (see `DockerSession`'s own doc comment for
    /// why role lives there, not in `BufferKind`).
    pub(super) fn action_key(&mut self, app: &mut App, keypress: KeyPress) -> bool {
        let Some(role) = self.focused_role(app) else {
            return false;
        };
        use DockerPaneRole::*;
        match (role, keypress.code) {
            (Containers, KeyCode::Char('s')) if keypress.mods == Mods::default() => {
                self.start_selected(app);
                app.wake_caret();
                true
            }
            (Containers, KeyCode::Char('S')) if keypress.mods == Mods::default() => {
                self.stop_selected(app);
                app.wake_caret();
                true
            }
            (Containers, KeyCode::Char('R')) if keypress.mods == Mods::default() => {
                self.restart_selected(app);
                app.wake_caret();
                true
            }
            (Containers, KeyCode::Char('l')) if keypress.mods == Mods::default() => {
                self.view_logs_selected(app);
                app.wake_caret();
                true
            }
            (Images, KeyCode::Char('r')) if keypress.mods == Mods::default() => {
                self.run_selected(app);
                app.wake_caret();
                true
            }
            (Containers, KeyCode::Char('d'))
            | (Images, KeyCode::Char('d'))
            | (Networks, KeyCode::Char('d'))
                if keypress.mods == Mods::default() =>
            {
                if let Some(entry) = self.entry_at_cursor(app) {
                    self.confirm_remove = Some(entry);
                }
                app.wake_caret();
                true
            }
            (_, KeyCode::Char('u')) if keypress.mods == Mods::default() => {
                self.refresh(app);
                app.wake_caret();
                true
            }
            (Containers | Images | Volumes | Networks, KeyCode::Char('x'))
                if keypress.mods == Mods::default() =>
            {
                self.menu_open = true;
                app.wake_caret();
                true
            }
            // Jump straight to the pane numbered `n` in its title bar
            // (`1. Containers`, `2. Images`, ...) -- a plain digit
            // here would otherwise just start an unused Vim count on
            // a non-editable panel buffer, so claiming it costs
            // nothing real editing would use.
            (_, KeyCode::Char(c)) if keypress.mods == Mods::default() && c.is_ascii_digit() => {
                if let Some(pane) = self.pane_by_number(c.to_digit(10).unwrap_or(0)) {
                    app.windows_mut().focus(pane);
                    self.sync_details_from_a_list(app);
                    app.wake_caret();
                }
                true
            }
            _ => false,
        }
    }

    /// Ordinary movement in one of the four left panes re-syncs the
    /// Status pane to whatever's now under the cursor -- pure
    /// formatting, no `docker` call, cheap enough every keystroke
    /// (mirrors `sync_completion`'s own "derive fresh from whatever's
    /// current" posture). Skipped while logs are being followed
    /// (`logs_container.is_some()`).
    fn sync_details_from_a_list(&mut self, app: &mut App) {
        if matches!(
            self.focused_role(app),
            Some(
                DockerPaneRole::Containers
                    | DockerPaneRole::Images
                    | DockerPaneRole::Volumes
                    | DockerPaneRole::Networks
            )
        ) && self
            .session
            .as_ref()
            .is_some_and(|s| s.logs_container.is_none())
        {
            self.sync_details(app);
        }
    }

    /// The Docker panel's contextual "view command options" popup (`x`
    /// on a Containers/Images/Volumes pane) -- mirrors `which_key_popup`'s
    /// shape, but its content is a small static per-`DockerPaneRole` list
    /// instead of `pending_hints()`. `None` whenever the popup isn't
    /// showing (`menu_open` is false), the focused pane isn't part of a
    /// Docker session, or that pane has no bindings of its own
    /// (Status/Logs).
    pub(super) fn menu_popup(
        &self,
        app: &App,
        window_width: f32,
        modeline_top: f32,
    ) -> Option<(fenix_window::Rect, RowSpans)> {
        if !self.menu_open {
            return None;
        }
        let bindings: &[(&str, &str)] = match self.focused_role(app)? {
            DockerPaneRole::Containers => &[
                ("s", "start"),
                ("S", "stop"),
                ("R", "restart"),
                ("l", "logs"),
                ("d", "remove"),
                ("u", "refresh"),
            ],
            DockerPaneRole::Images => &[("r", "run"), ("d", "remove"), ("u", "refresh")],
            DockerPaneRole::Volumes => &[("u", "refresh")],
            DockerPaneRole::Networks => &[("d", "remove"), ("u", "refresh")],
            DockerPaneRole::Status | DockerPaneRole::Logs => return None,
        };

        let (char_width, line_height) = match &app.text {
            Some(text) => (text.char_width(), text.line_height()),
            None => (text::CHAR_WIDTH, text::LINE_HEIGHT),
        };

        let max_rows = popup::max_rows(
            modeline_top,
            text::WHICH_KEY_MARGIN,
            line_height,
            WHICH_KEY_PADDING,
        )
        .max(1);
        let shown_count = bindings.len().min(max_rows);

        const KEY_COLUMN_CHARS: usize = 6;
        let longest_label = bindings[..shown_count]
            .iter()
            .map(|(_, label)| label.chars().count())
            .max()
            .unwrap_or(0);
        let content_chars = KEY_COLUMN_CHARS + longest_label + 1;
        let max_width =
            (window_width - 2.0 * text::WHICH_KEY_MARGIN).max(text::WHICH_KEY_MIN_WIDTH);
        let width = (content_chars as f32 * char_width + WHICH_KEY_PADDING).clamp(
            text::WHICH_KEY_MIN_WIDTH,
            text::WHICH_KEY_MAX_WIDTH.min(max_width),
        );

        let theme = app.theme;
        let mut spans = Vec::new();
        for (i, (key, label)) in bindings[..shown_count].iter().enumerate() {
            if i > 0 {
                spans.push(("\n".to_string(), theme.fg_modeline, false));
            }
            spans.push((format!("{key:<KEY_COLUMN_CHARS$}"), theme.caret_text, false));
            spans.push(((*label).to_string(), theme.fg_modeline, false));
        }

        let height = shown_count as f32 * line_height + WHICH_KEY_PADDING;
        let rect = popup::resolve(
            popup::Anchor::TopRight {
                margin: text::WHICH_KEY_MARGIN,
            },
            width,
            height,
            window_width,
            modeline_top,
        );
        Some((rect, spans))
    }

    /// Whether `buffer` is one of the open session's six.
    fn owns_buffer(&self, buffer: BufferId) -> bool {
        self.session.as_ref().is_some_and(|session| {
            [
                session.containers_buffer,
                session.images_buffer,
                session.volumes_buffer,
                session.networks_buffer,
                session.status_buffer,
                session.logs_buffer,
            ]
            .contains(&buffer)
        })
    }
}

impl Feature for DockerHost {
    fn spec(&self) -> &'static FeatureSpec {
        &SPEC
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn on_event(&mut self, app: &mut App, payload: Box<dyn Any + Send>) {
        let Ok(event) = payload.downcast::<DockerEvent>() else {
            return;
        };
        match *event {
            DockerEvent::Stats(stats) => self.apply_stats(app, stats),
            DockerEvent::LogLine(buffer_id, line) => self.append_log_line(app, buffer_id, line),
            DockerEvent::LogEnded(buffer_id) => self.finish_log_stream(app, buffer_id),
        }
    }

    /// The `x` menu is purely informational, not a capturing prompt --
    /// cleared on *every* keypress before that keypress is dispatched,
    /// so pressing any key both dismisses the popup and still does
    /// whatever that key would otherwise do (real Lazydocker's own
    /// "view command options" convention).
    fn key_arrived(&mut self) {
        self.menu_open = false;
    }

    /// An armed remove confirmation (`d` on a container/image/network
    /// row) captures the very next key -- `y` confirms, anything else
    /// cancels (see `confirm_key`).
    fn capture_key(&mut self, app: &mut App, key: KeyPress) -> bool {
        if self.confirm_remove.is_none() {
            return false;
        }
        self.confirm_key(app, key);
        true
    }

    fn pane_key(&mut self, app: &mut App, key: KeyPress) -> bool {
        self.action_key(app, key)
    }

    fn after_vim_key(&mut self, app: &mut App) {
        self.sync_details_from_a_list(app);
    }

    fn holds_focused_pane(&self, app: &App) -> bool {
        self.focused_role(app).is_some()
    }

    fn blocks_jumplist(&self) -> bool {
        self.session.is_some()
    }

    /// One pane is enough to identify the session: it builds all of its
    /// panes in one workspace, so they're either all in this frame or
    /// none of them are.
    fn close_panes(&mut self, app: &mut App, panes: &[fenix_window::WindowId]) {
        if self
            .session
            .as_ref()
            .is_some_and(|session| panes.contains(&session.containers_pane))
        {
            self.close(app);
        }
    }

    /// Closing one of the session's buffers closes the whole session,
    /// not just that buffer -- otherwise a pane would be left pointing
    /// at a closed `BufferId`.
    fn close_buffer(&mut self, app: &mut App, buffer: BufferId) -> bool {
        if !self.owns_buffer(buffer) {
            return false;
        }
        self.close(app);
        true
    }

    fn prompt_text(&self) -> Option<String> {
        self.confirm_text()
    }

    fn highlights(
        &self,
        ob: &OpenBuffer,
        buffer: BufferId,
        first_row: usize,
        rows: usize,
        theme: &Theme,
    ) -> Option<Highlights> {
        if ob.kind != BufferKind::Docker {
            return None;
        }
        Some(highlights_for_visible_range(
            ob,
            self.lines.get(&buffer).map(Vec::as_slice),
            first_row,
            rows,
            theme,
        ))
    }

    fn popup(
        &self,
        app: &App,
        window_width: f32,
        modeline_top: f32,
    ) -> Option<(popup::PopupId, fenix_window::Rect, RowSpans)> {
        self.menu_popup(app, window_width, modeline_top)
            .map(|(rect, spans)| (popup::PopupId::Feature(SPEC.id), rect, spans))
    }
}

/// The Docker panel's equivalent of `dashboard_highlights_for_visible_
/// range` -- same shape, built from `docker_panel::render`'s own per-line
/// `DockerLine` metadata instead.
fn highlights_for_visible_range(
    ob: &OpenBuffer,
    lines: Option<&[Option<docker_panel::DockerLine>]>,
    render_base_line: usize,
    rows: usize,
    theme: &Theme,
) -> Highlights {
    let Some(lines) = lines else {
        return Vec::new();
    };
    let visual_lines = ob.buffer.visual_line_count();
    let mut ranges = Vec::new();
    for line in render_base_line..(render_base_line + rows).min(visual_lines) {
        let Some(Some(meta)) = lines.get(line) else {
            continue;
        };
        let start = ob.buffer.line_start_char(line);
        let len = ob.buffer.line_len(line);
        let line_start_byte = ob.buffer.char_to_byte(start);
        let line_end_byte = ob.buffer.char_to_byte(start + len);
        match meta.style {
            docker_panel::DockerLineStyle::Empty => {
                ranges.push((line_start_byte..line_end_byte, theme.gutter_fg));
            }
            docker_panel::DockerLineStyle::Container
            | docker_panel::DockerLineStyle::Image
            | docker_panel::DockerLineStyle::Volume
            | docker_panel::DockerLineStyle::Network
            | docker_panel::DockerLineStyle::Detail => {
                if let Some(dim_from) = meta.dim_from {
                    let dim_start_byte = ob.buffer.char_to_byte(start + dim_from);
                    ranges.push((dim_start_byte..line_end_byte, theme.gutter_fg));
                }
            }
        }
        if let Some((badge_len, color)) = meta.badge {
            let badge_end_byte = ob.buffer.char_to_byte(start + badge_len);
            ranges.push((line_start_byte..badge_end_byte, badge_color(color, theme)));
        }
    }
    ranges
}

/// Maps `docker_panel`'s theme-agnostic `DockerBadgeColor` bucket to a
/// real theme color.
fn badge_color(color: docker_panel::DockerBadgeColor, theme: &Theme) -> glyphon::Color {
    match color {
        docker_panel::DockerBadgeColor::Good => theme.git_staged,
        docker_panel::DockerBadgeColor::Warn => theme.git_modified,
        docker_panel::DockerBadgeColor::Bad => theme.git_conflicted,
        docker_panel::DockerBadgeColor::Neutral => theme.gutter_fg,
    }
}

/// Test-only ways in: `App` no longer has Docker's fields or methods, so
/// tests reach the host through these, under the names the tests have
/// always used.
#[cfg(test)]
impl App {
    pub(super) fn docker(&self) -> &DockerHost {
        self.features
            .get::<DockerHost>()
            .expect("Docker is a built-in feature")
    }

    pub(super) fn docker_mut(&mut self) -> &mut DockerHost {
        self.features
            .get_mut::<DockerHost>()
            .expect("Docker is a built-in feature")
    }

    fn with_docker<R>(&mut self, f: impl FnOnce(&mut DockerHost, &mut App) -> R) -> R {
        self.with_feature::<DockerHost, _>(f)
            .expect("Docker is a built-in feature")
    }

    pub(super) fn open_docker_panel(&mut self) {
        self.with_docker(|docker, app| docker.open(app));
    }

    pub(super) fn docker_focused_role(&self) -> Option<DockerPaneRole> {
        self.docker().focused_role(self)
    }

    fn docker_pane_by_number(&self, n: u32) -> Option<fenix_window::WindowId> {
        self.docker().pane_by_number(n)
    }

    fn docker_entry_at_cursor(&self) -> Option<docker_panel::DockerEntry> {
        self.docker().entry_at_cursor(self)
    }

    fn docker_confirm_text(&self) -> Option<String> {
        self.docker().confirm_text()
    }

    fn docker_menu_popup(
        &self,
        window_width: f32,
        modeline_top: f32,
    ) -> Option<(fenix_window::Rect, RowSpans)> {
        self.docker().menu_popup(self, window_width, modeline_top)
    }

    fn set_docker_buffer(&mut self, id: BufferId, panel: docker_panel::DockerPanel) {
        self.with_docker(|docker, app| docker.set_buffer(app, id, panel));
    }

    fn apply_docker_stats(&mut self, stats: Vec<fenix_docker::ContainerStat>) {
        self.with_docker(|docker, app| docker.apply_stats(app, stats));
    }

    fn append_docker_log_line(&mut self, buffer_id: BufferId, line: String) {
        self.with_docker(|docker, app| docker.append_log_line(app, buffer_id, line));
    }

    fn finish_docker_log_stream(&mut self, buffer_id: BufferId) {
        self.with_docker(|docker, app| docker.finish_log_stream(app, buffer_id));
    }

    fn docker_sync_details(&mut self) {
        self.with_docker(|docker, app| docker.sync_details(app));
    }

    fn docker_session_close(&mut self) {
        self.with_docker(|docker, app| docker.close(app));
    }

    fn docker_view_logs_selected(&mut self) {
        self.with_docker(|docker, app| docker.view_logs_selected(app));
    }

    fn docker_start_selected(&mut self) {
        self.with_docker(|docker, app| docker.start_selected(app));
    }

    fn docker_stop_selected(&mut self) {
        self.with_docker(|docker, app| docker.stop_selected(app));
    }

    fn docker_restart_selected(&mut self) {
        self.with_docker(|docker, app| docker.restart_selected(app));
    }

    fn docker_run_selected(&mut self) {
        self.with_docker(|docker, app| docker.run_selected(app));
    }

    fn docker_confirm_key(&mut self, keypress: KeyPress) {
        self.with_docker(|docker, app| docker.confirm_key(app, keypress));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-built `fenix_docker::Container`, for tests that need real
    /// container values without a working `docker` -- mirrors `docker_
    /// panel.rs`'s own private test helper of the same shape.
    fn container(id: &str, name: &str) -> fenix_docker::Container {
        fenix_docker::Container {
            id: id.to_string(),
            name: name.to_string(),
            image: "nginx:latest".to_string(),
            status: "Up 3 days".to_string(),
            state: "running".to_string(),
        }
    }

    #[test]
    fn open_docker_panel_opens_a_tagged_buffer_with_line_metadata() {
        // `docker` itself is unreachable in a headless/sandboxed test
        // environment (no daemon socket access) -- `list_containers`/
        // `list_images` degrade to empty `Vec`s per their own "never
        // fails" contract, so this only asserts the wiring (buffer kind,
        // line-metadata table), not real container/image data.
        let mut app = App::with_file(None);
        app.open_docker_panel();
        assert_eq!(app.open().kind, BufferKind::Docker);
        let id = app.focused_buffer_id();
        assert!(app.docker().lines.contains_key(&id));
    }

    #[test]
    fn open_docker_panel_replaces_the_focused_pane_with_a_fresh_docker_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        std::fs::write(&a, "hello").unwrap();
        let mut app = App::with_file(None);
        app.test_open_path(&a);
        let a_id = app.focused_buffer_id();

        app.open_docker_panel();

        assert_ne!(app.focused_buffer_id(), a_id);
        assert_eq!(app.open().kind, BufferKind::Docker);
    }

    #[test]
    fn docker_stats_poller_ticks_at_least_once_and_stops_promptly_on_drop() {
        // No real `EventLoopProxy` needed -- `DockerStatsPoller::spawn`
        // takes a plain callback (see its own doc comment for why), so
        // this exercises the actual thread-lifecycle logic (spawn, tick,
        // stop-check, join-on-drop) directly with a channel standing in
        // for "deliver this event somewhere," and a stand-in stats source
        // so the timing doesn't depend on whether a Docker engine is
        // running (a real `docker stats` then takes seconds).
        let (tx, rx) = std::sync::mpsc::channel::<DockerEvent>();
        let stat = fenix_docker::ContainerStat {
            container_id: "abc123".to_string(),
            cpu_percent: "1%".to_string(),
            mem_usage: "1MiB".to_string(),
        };
        let poller = DockerStatsPoller::spawn(
            move || vec![stat.clone()],
            move |event| tx.send(event).is_ok(),
        );
        let received = rx
            .recv_timeout(Duration::from_secs(3))
            .expect("poller should tick at least once");
        assert!(
            matches!(&received, DockerEvent::Stats(stats) if stats.len() == 1 && stats[0].container_id == "abc123")
        );

        let before_drop = Instant::now();
        drop(poller);
        // Bounded by `STATS_POLL_STOP_CHECK` (100ms), not the full
        // `STATS_POLL_INTERVAL` (2s) -- generous margin for test-host
        // scheduling jitter, but well under a full interval.
        assert!(
            before_drop.elapsed() < Duration::from_secs(1),
            "drop should join promptly, not wait out a full poll interval"
        );
    }

    #[test]
    #[ignore = "subprocess fixture, invoked by the Docker log follower test"]
    fn docker_log_fixture() {
        if std::env::var("FENIX_DOCKER_LOG_FIXTURE").as_deref() != Ok("1") {
            return;
        }
        use std::io::Write;
        std::io::stdout().write_all(b"line1\nline2\n").unwrap();
        std::io::stdout().flush().unwrap();
        std::thread::sleep(Duration::from_secs(100));
    }

    #[test]
    fn docker_log_follower_delivers_lines_and_stops_promptly_on_drop() {
        // A real subprocess standing in for `docker logs -f` (a harmless
        // long-running command, per the plan's own "never assert on
        // real docker output" posture) -- prints two lines immediately,
        // then sleeps long enough that `drop`'s `kill()` is what has to
        // end it, not the process exiting on its own.
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "app::docker_host::tests::docker_log_fixture",
                "--nocapture",
            ])
            .env("FENIX_DOCKER_LOG_FIXTURE", "1")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("the Rust log fixture should spawn");

        let buffer_id = App::with_file(None).focused_buffer_id();
        let (tx, rx) = std::sync::mpsc::channel::<DockerEvent>();
        let follower =
            DockerLogFollower::spawn(child, buffer_id, move |event| tx.send(event).is_ok());

        // The test harness can emit its own preamble before the fixture's output.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut lines = Vec::new();
        while lines.len() < 2 {
            match rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("fixture should deliver both lines")
            {
                DockerEvent::LogLine(id, line) => {
                    assert_eq!(id, buffer_id);
                    if line == "line1" || line == "line2" {
                        lines.push(line);
                    }
                }
                other => panic!("expected a LogLine event, got {other:?}"),
            }
        }
        assert_eq!(lines, ["line1", "line2"]);

        let before_drop = Instant::now();
        drop(follower);
        assert!(
            before_drop.elapsed() < Duration::from_secs(2),
            "drop should kill+join promptly, not wait out the child's own 100s sleep"
        );
    }

    #[test]
    fn append_docker_log_line_auto_follows_a_pane_that_was_already_at_the_last_line() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let logs_buffer = app.docker().session.as_ref().unwrap().logs_buffer;
        let logs_pane = app.docker().session.as_ref().unwrap().logs_pane;
        app.set_docker_buffer(
            logs_buffer,
            docker_panel::DockerPanel {
                text: "line1\nline2\n".to_string(),
                lines: Vec::new(),
            },
        );
        // Put the cursor on the last (real) line -- "line2".
        let last_line = app
            .buffers
            .get(logs_buffer)
            .unwrap()
            .buffer
            .visual_line_count()
            - 1;
        let start = app
            .buffers
            .get(logs_buffer)
            .unwrap()
            .buffer
            .line_start_char(last_line);
        app.pane_state_mut(logs_pane).cursor = Cursor {
            char_idx: start,
            sticky_col: 0,
        };

        app.append_docker_log_line(logs_buffer, "line3".to_string());

        let text = app.buffers.get(logs_buffer).unwrap().buffer.text();
        assert!(text.contains("line3"));
        let new_last_line = app
            .buffers
            .get(logs_buffer)
            .unwrap()
            .buffer
            .visual_line_count()
            - 1;
        let (cursor_line, _) = app
            .buffers
            .get(logs_buffer)
            .unwrap()
            .buffer
            .line_col(&app.pane_state(logs_pane).cursor);
        assert_eq!(
            cursor_line, new_last_line,
            "cursor should have followed to the new last line"
        );
    }

    #[test]
    fn append_docker_log_line_leaves_a_pane_alone_if_the_user_scrolled_away() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let logs_buffer = app.docker().session.as_ref().unwrap().logs_buffer;
        let logs_pane = app.docker().session.as_ref().unwrap().logs_pane;
        app.set_docker_buffer(
            logs_buffer,
            docker_panel::DockerPanel {
                text: "line1\nline2\nline3\n".to_string(),
                lines: Vec::new(),
            },
        );
        // Cursor on the *first* line, not the last -- simulates having
        // scrolled up to read earlier output.
        app.pane_state_mut(logs_pane).cursor = Cursor {
            char_idx: 0,
            sticky_col: 0,
        };

        app.append_docker_log_line(logs_buffer, "line4".to_string());

        let (cursor_line, _) = app
            .buffers
            .get(logs_buffer)
            .unwrap()
            .buffer
            .line_col(&app.pane_state(logs_pane).cursor);
        assert_eq!(
            cursor_line, 0,
            "cursor should stay put, not get yanked to the new bottom"
        );
    }

    #[test]
    fn append_docker_log_line_ignores_a_stray_event_for_a_buffer_that_is_not_the_current_logs_buffer(
    ) {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let containers_buffer = app.docker().session.as_ref().unwrap().containers_buffer;
        let before = app.buffers.get(containers_buffer).unwrap().buffer.text();

        app.append_docker_log_line(containers_buffer, "should be ignored".to_string());

        let after = app.buffers.get(containers_buffer).unwrap().buffer.text();
        assert_eq!(before, after);
    }

    #[test]
    fn finish_docker_log_stream_appends_a_marker_and_clears_the_follower() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let logs_buffer = app.docker().session.as_ref().unwrap().logs_buffer;

        app.finish_docker_log_stream(logs_buffer);

        let text = app.buffers.get(logs_buffer).unwrap().buffer.text();
        assert!(text.contains("log stream ended"));
        assert!(app
            .docker()
            .session
            .as_ref()
            .unwrap()
            .log_follower
            .is_none());
    }

    #[test]
    fn apply_docker_stats_merges_into_the_containers_pane_without_a_session_being_a_no_op() {
        // No `docker_session` open -- must not panic.
        let mut app = App::with_file(None);
        app.apply_docker_stats(vec![]);
    }

    #[test]
    fn apply_docker_stats_merges_a_matching_stat_into_the_status_pane() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let containers_buffer = app.docker().session.as_ref().unwrap().containers_buffer;
        let status_buffer = app.docker().session.as_ref().unwrap().status_buffer;
        app.docker_mut().session.as_mut().unwrap().containers = vec![container("abc123", "web")];
        app.docker_mut().lines.insert(
            containers_buffer,
            vec![Some(docker_panel::DockerLine {
                style: docker_panel::DockerLineStyle::Container,
                entry: Some(docker_panel::DockerEntry::Container("abc123".to_string())),
                dim_from: None,
                badge: None,
            })],
        );
        app.test_set_cursor(Cursor {
            char_idx: 0,
            sticky_col: 0,
        });

        app.apply_docker_stats(vec![fenix_docker::ContainerStat {
            container_id: "abc123".to_string(),
            cpu_percent: "3.5%".to_string(),
            mem_usage: "20MiB".to_string(),
        }]);

        let text = app.buffers.get(status_buffer).unwrap().buffer.text();
        assert!(text.contains("CPU: 3.5%"));
    }

    #[test]
    fn open_docker_panel_creates_a_six_pane_session_with_titles() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let session = app.docker().session.as_ref().unwrap();
        let panes = [
            session.containers_pane,
            session.images_pane,
            session.volumes_pane,
            session.networks_pane,
            session.status_pane,
            session.logs_pane,
        ];
        let distinct: std::collections::HashSet<_> = panes.iter().collect();
        assert_eq!(
            distinct.len(),
            6,
            "every pane should be a distinct WindowId"
        );
        assert_eq!(app.windows().window_count(), 6);
        assert_eq!(
            app.pane_titles
                .get(&session.containers_pane)
                .map(String::as_str),
            Some("1. Containers")
        );
        assert_eq!(
            app.pane_titles
                .get(&session.images_pane)
                .map(String::as_str),
            Some("2. Images")
        );
        assert_eq!(
            app.pane_titles
                .get(&session.volumes_pane)
                .map(String::as_str),
            Some("3. Volumes")
        );
        assert_eq!(
            app.pane_titles
                .get(&session.networks_pane)
                .map(String::as_str),
            Some("4. Networks")
        );
        assert_eq!(
            app.pane_titles
                .get(&session.status_pane)
                .map(String::as_str),
            Some("5. Status")
        );
        assert_eq!(
            app.pane_titles.get(&session.logs_pane).map(String::as_str),
            Some("6. Logs")
        );
        assert_eq!(app.focused_pane_id(), session.containers_pane);
    }

    #[test]
    fn open_docker_panel_layout_gives_containers_the_biggest_share_of_the_left_column() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let session = app.docker().session.as_ref().unwrap();
        let bounds = fenix_window::Rect {
            x: 0.0,
            y: 0.0,
            w: 1000.0,
            h: 900.0,
        };
        let layout = app.windows().layout(bounds);
        let rect_for = |id: fenix_window::WindowId| {
            layout
                .iter()
                .find(|(p, _)| *p == id)
                .map(|(_, r)| *r)
                .unwrap()
        };
        let containers_rect = rect_for(session.containers_pane);
        let images_rect = rect_for(session.images_pane);
        let volumes_rect = rect_for(session.volumes_pane);
        let networks_rect = rect_for(session.networks_pane);
        let status_rect = rect_for(session.status_pane);
        let logs_rect = rect_for(session.logs_pane);

        // Left column is narrower than the right (Status/Logs) column.
        assert!(containers_rect.w < status_rect.w);
        assert!(containers_rect.w < logs_rect.w);
        // Containers gets roughly half the left column's height,
        // Images/Volumes/Networks split the other half evenly.
        assert!(containers_rect.h > images_rect.h);
        assert!((images_rect.h - volumes_rect.h).abs() < 1.0);
        assert!((volumes_rect.h - networks_rect.h).abs() < 1.0);
        // Logs gets a bigger share of the right column than Status.
        assert!(logs_rect.h > status_rect.h);
    }

    #[test]
    fn reopening_the_docker_panel_refocuses_the_existing_session_instead_of_creating_a_second() {
        // `WindowId`s are only unique *within* one workspace's tree, so
        // this deliberately checks *workspace* identity (via `docker_
        // focused_role`, which is itself workspace-aware -- see its own
        // doc comment) rather than comparing raw `WindowId`s across
        // workspaces, which could false-positive-match by coincidence.
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let session_workspace_index = app.docker().session.as_ref().unwrap().workspace_index;
        let workspace_count_after_first = app.workspaces.len();

        // Switch away, then re-open -- should refocus, not duplicate.
        app.new_workspace();
        assert_ne!(app.workspaces.active_index(), session_workspace_index);
        assert_eq!(app.docker_focused_role(), None);

        app.open_docker_panel();
        assert_eq!(app.workspaces.active_index(), session_workspace_index);
        assert_eq!(app.docker_focused_role(), Some(DockerPaneRole::Containers));
        // One more workspace than after the first open (the one created
        // by `new_workspace` above), not two more (no second session).
        assert_eq!(app.workspaces.len(), workspace_count_after_first + 1);
    }

    #[test]
    fn docker_focused_role_identifies_each_pane_and_none_outside_the_session() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let session = app.docker().session.as_ref().unwrap();
        let (containers, images, volumes, networks, status, logs) = (
            session.containers_pane,
            session.images_pane,
            session.volumes_pane,
            session.networks_pane,
            session.status_pane,
            session.logs_pane,
        );

        assert_eq!(app.docker_focused_role(), Some(DockerPaneRole::Containers));
        app.windows_mut().focus(images);
        assert_eq!(app.docker_focused_role(), Some(DockerPaneRole::Images));
        app.windows_mut().focus(volumes);
        assert_eq!(app.docker_focused_role(), Some(DockerPaneRole::Volumes));
        app.windows_mut().focus(networks);
        assert_eq!(app.docker_focused_role(), Some(DockerPaneRole::Networks));
        app.windows_mut().focus(status);
        assert_eq!(app.docker_focused_role(), Some(DockerPaneRole::Status));
        app.windows_mut().focus(logs);
        assert_eq!(app.docker_focused_role(), Some(DockerPaneRole::Logs));
        app.windows_mut().focus(containers);
        assert_eq!(app.docker_focused_role(), Some(DockerPaneRole::Containers));

        app.docker_session_close();
        assert_eq!(app.docker_focused_role(), None);
    }

    // `App::handle_key` itself can't be driven directly in a unit test
    // (needs a real winit `KeyEvent`/`ActiveEventLoop`, see the comment
    // above `vim_pulse_event_yields_a_renderable_pulse_overlay`) -- this
    // covers the lookup its digit-key routing actually depends on,
    // matching the same title-bar numbering `open_docker_panel` sets
    // (`1. Containers`, `2. Images`, ...).
    #[test]
    fn docker_pane_by_number_matches_each_titles_own_number() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let session = app.docker().session.as_ref().unwrap();
        let (containers, images, volumes, networks, status, logs) = (
            session.containers_pane,
            session.images_pane,
            session.volumes_pane,
            session.networks_pane,
            session.status_pane,
            session.logs_pane,
        );

        assert_eq!(app.docker_pane_by_number(1), Some(containers));
        assert_eq!(app.docker_pane_by_number(2), Some(images));
        assert_eq!(app.docker_pane_by_number(3), Some(volumes));
        assert_eq!(app.docker_pane_by_number(4), Some(networks));
        assert_eq!(app.docker_pane_by_number(5), Some(status));
        assert_eq!(app.docker_pane_by_number(6), Some(logs));
        assert_eq!(app.docker_pane_by_number(7), None);
        assert_eq!(app.docker_pane_by_number(0), None);

        app.docker_session_close();
        assert_eq!(app.docker_pane_by_number(1), None);
    }

    #[test]
    fn docker_sync_details_renders_the_selected_container_into_the_status_pane() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let session_containers_buffer = app.docker().session.as_ref().unwrap().containers_buffer;
        let status_buffer = app.docker().session.as_ref().unwrap().status_buffer;

        // Seed a fake container both in the cached session list (what
        // `docker_sync_details` actually reads) and in `docker_lines`
        // (what `docker_entry_at_cursor` reads) -- independent of
        // whatever real `docker` returned in this sandboxed test env.
        let fake = fenix_docker::Container {
            id: "abc123".to_string(),
            name: "web".to_string(),
            image: "nginx:latest".to_string(),
            status: "Up 3 days".to_string(),
            state: "running".to_string(),
        };
        app.docker_mut().session.as_mut().unwrap().containers = vec![fake];
        app.docker_mut().lines.insert(
            session_containers_buffer,
            vec![Some(docker_panel::DockerLine {
                style: docker_panel::DockerLineStyle::Container,
                entry: Some(docker_panel::DockerEntry::Container("abc123".to_string())),
                dim_from: None,
                badge: None,
            })],
        );
        app.test_set_cursor(Cursor {
            char_idx: 0,
            sticky_col: 0,
        });

        app.docker_sync_details();

        let status_text = app.buffers.get(status_buffer).unwrap().buffer.text();
        assert!(status_text.contains("Name: web"));
    }

    #[test]
    fn docker_session_close_removes_all_six_buffers_and_the_workspace() {
        let mut app = App::with_file(None);
        let workspace_count_before = app.workspaces.len();
        app.open_docker_panel();
        let session = app.docker().session.as_ref().unwrap();
        let buffers = [
            session.containers_buffer,
            session.images_buffer,
            session.volumes_buffer,
            session.networks_buffer,
            session.status_buffer,
            session.logs_buffer,
        ];

        app.docker_session_close();

        assert!(app.docker().session.is_none());
        assert_eq!(app.workspaces.len(), workspace_count_before);
        for id in buffers {
            assert!(app.buffers.get(id).is_none());
            assert!(!app.docker().lines.contains_key(&id));
        }
    }

    #[test]
    fn kill_buffer_on_a_docker_pane_closes_the_whole_session_not_just_one_buffer() {
        let mut app = App::with_file(None);
        let workspace_count_before = app.workspaces.len();
        app.open_docker_panel();

        app.kill_buffer();

        assert!(app.docker().session.is_none());
        assert_eq!(app.workspaces.len(), workspace_count_before);
    }

    #[test]
    fn docker_action_keys_on_a_non_entry_line_are_a_no_op() {
        // Cursor sits on line 0 (the header), which has no `entry` --
        // every action should just do nothing rather than panic.
        let mut app = App::with_file(None);
        app.open_docker_panel();
        app.docker_start_selected();
        app.docker_stop_selected();
        app.docker_restart_selected();
        app.docker_run_selected();
        assert_eq!(app.open().kind, BufferKind::Docker);
        assert!(app.docker().confirm_remove.is_none());
    }

    #[test]
    fn docker_entry_at_cursor_reads_the_line_under_the_cursor() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let id = app.focused_buffer_id();
        // Force a known entry at line 0, independent of whatever real
        // `docker` output (or lack thereof) actually rendered there.
        app.docker_mut().lines.insert(
            id,
            vec![Some(docker_panel::DockerLine {
                style: docker_panel::DockerLineStyle::Container,
                entry: Some(docker_panel::DockerEntry::Container("abc123".to_string())),
                dim_from: None,
                badge: None,
            })],
        );
        app.test_set_cursor(Cursor {
            char_idx: 0,
            sticky_col: 0,
        });

        assert_eq!(
            app.docker_entry_at_cursor(),
            Some(docker_panel::DockerEntry::Container("abc123".to_string()))
        );
    }

    #[test]
    fn docker_view_logs_selected_updates_the_logs_pane_in_place() {
        // `docker logs` itself is unreachable in this sandboxed test
        // environment (no daemon socket access) -- it'll come back an
        // `Err`, which `docker_view_logs_selected` still renders into
        // the Logs buffer rather than silently doing nothing. The
        // point of this test is the wiring (focus stays on Containers,
        // Logs gets rewritten), not the specific log text.
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let containers_id = app.focused_buffer_id();
        app.docker_mut().lines.insert(
            containers_id,
            vec![Some(docker_panel::DockerLine {
                style: docker_panel::DockerLineStyle::Container,
                entry: Some(docker_panel::DockerEntry::Container("abc123".to_string())),
                dim_from: None,
                badge: None,
            })],
        );
        app.test_set_cursor(Cursor {
            char_idx: 0,
            sticky_col: 0,
        });
        let logs_id = app.docker().session.as_ref().unwrap().logs_buffer;

        app.docker_view_logs_selected();

        assert_eq!(app.focused_buffer_id(), containers_id); // focus never left Containers
        let logs_text = app.buffers.get(logs_id).unwrap().buffer.text();
        assert!(!logs_text.is_empty());
    }

    #[test]
    fn docker_view_logs_selected_on_an_image_row_is_a_no_op() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let docker_id = app.focused_buffer_id();
        app.docker_mut().lines.insert(
            docker_id,
            vec![Some(docker_panel::DockerLine {
                style: docker_panel::DockerLineStyle::Image,
                entry: Some(docker_panel::DockerEntry::Image("sha256:dead".to_string())),
                dim_from: None,
                badge: None,
            })],
        );
        app.test_set_cursor(Cursor {
            char_idx: 0,
            sticky_col: 0,
        });

        app.docker_view_logs_selected();

        assert_eq!(app.focused_buffer_id(), docker_id);
        assert_eq!(app.open().kind, BufferKind::Docker);
    }

    #[test]
    fn docker_confirm_remove_shows_a_prompt_and_a_non_y_key_cancels_without_removing() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let id = app.focused_buffer_id();
        app.docker_mut().lines.insert(
            id,
            vec![Some(docker_panel::DockerLine {
                style: docker_panel::DockerLineStyle::Image,
                entry: Some(docker_panel::DockerEntry::Image("sha256:dead".to_string())),
                dim_from: None,
                badge: None,
            })],
        );
        app.test_set_cursor(Cursor {
            char_idx: 0,
            sticky_col: 0,
        });

        let entry = app.docker_entry_at_cursor().unwrap();
        app.docker_mut().confirm_remove = Some(entry);
        assert!(app.docker_confirm_text().unwrap().contains("image"));
        assert!(app
            .active_prompt_text()
            .unwrap()
            .contains("Remove this image?"));

        app.docker_confirm_key(KeyPress::char('n'));
        assert!(app.docker().confirm_remove.is_none());
    }

    #[test]
    fn docker_confirm_remove_with_y_clears_the_prompt_and_refreshes() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let id = app.focused_buffer_id();
        app.docker_mut().lines.insert(
            id,
            vec![Some(docker_panel::DockerLine {
                style: docker_panel::DockerLineStyle::Container,
                entry: Some(docker_panel::DockerEntry::Container("abc123".to_string())),
                dim_from: None,
                badge: None,
            })],
        );
        app.test_set_cursor(Cursor {
            char_idx: 0,
            sticky_col: 0,
        });
        app.docker_mut().confirm_remove = app.docker_entry_at_cursor();

        // `docker rm` itself fails in this sandboxed environment (no
        // daemon access) -- the point of this test is that confirming
        // clears the armed state and refreshes without panicking, not
        // that a real removal happened.
        app.docker_confirm_key(KeyPress::char('y'));
        assert!(app.docker().confirm_remove.is_none());
        assert_eq!(app.open().kind, BufferKind::Docker);
    }

    #[test]
    fn docker_menu_popup_is_none_when_not_open() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        assert!(app.docker_menu_popup(800.0, 580.0).is_none());
    }

    #[test]
    fn docker_menu_popup_is_none_outside_a_docker_session() {
        let mut app = App::with_file(None);
        app.docker_mut().menu_open = true;
        assert!(app.docker_menu_popup(800.0, 580.0).is_none());
    }

    #[test]
    fn docker_menu_popup_lists_the_containers_pane_bindings() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        app.docker_mut().menu_open = true;
        let (_, spans) = app.docker_menu_popup(800.0, 580.0).unwrap();
        let text: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
        for key in ["s", "S", "R", "l", "d", "u"] {
            assert!(
                text.contains(key),
                "expected the Containers menu to mention '{key}': {text}"
            );
        }
        assert!(text.contains("remove"));
        assert!(text.contains("refresh"));
    }

    #[test]
    fn docker_menu_popup_lists_the_images_pane_bindings() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let images_pane = app.docker().session.as_ref().unwrap().images_pane;
        app.windows_mut().focus(images_pane);
        app.docker_mut().menu_open = true;
        let (_, spans) = app.docker_menu_popup(800.0, 580.0).unwrap();
        let text: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
        assert!(text.contains("run"));
        assert!(text.contains("remove"));
        assert!(text.contains("refresh"));
    }

    #[test]
    fn docker_menu_popup_lists_the_networks_pane_bindings() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let networks_pane = app.docker().session.as_ref().unwrap().networks_pane;
        app.windows_mut().focus(networks_pane);
        app.docker_mut().menu_open = true;
        let (_, spans) = app.docker_menu_popup(800.0, 580.0).unwrap();
        let text: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
        assert!(text.contains("remove"));
        assert!(text.contains("refresh"));
    }

    #[test]
    fn docker_menu_popup_is_none_on_the_status_and_logs_panes() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let session = app.docker().session.as_ref().unwrap();
        let (status_pane, logs_pane) = (session.status_pane, session.logs_pane);
        app.docker_mut().menu_open = true;
        app.windows_mut().focus(status_pane);
        assert!(app.docker_menu_popup(800.0, 580.0).is_none());
        app.windows_mut().focus(logs_pane);
        assert!(app.docker_menu_popup(800.0, 580.0).is_none());
    }

    // Through the `Feature` hooks, the way `App` reaches Docker now.

    #[test]
    fn docker_contributes_its_commands_and_leader_keys_from_its_spec() {
        let registry = CommandRegistry::with_builtins();
        let find = |id: &str| {
            registry
                .iter()
                .find(|c| c.id == id)
                .unwrap_or_else(|| panic!("{id} is registered"))
        };
        assert!(find("docker.open").external());
        assert!(!find("docker.build").external());
        assert!(!find("docker.close").external());
        let keys = keymap::leader_keys();
        assert_eq!(keys.get("docker.open").map(String::as_str), Some("SPC d d"));
        assert_eq!(
            keys.get("docker.build").map(String::as_str),
            Some("SPC d b")
        );
        assert_eq!(
            keys.get("docker.close").map(String::as_str),
            Some("SPC d q")
        );
    }

    #[test]
    fn docker_open_runs_from_the_control_channel_and_reopening_refocuses() {
        let mut app = App::with_file(None);
        let registry = CommandRegistry::with_builtins();
        registry.run_external(&mut app, "docker.open").unwrap();
        let workspaces = app.workspaces.len();
        assert_eq!(app.docker_focused_role(), Some(DockerPaneRole::Containers));
        registry.run_external(&mut app, "docker.open").unwrap();
        assert_eq!(app.workspaces.len(), workspaces);
        assert_eq!(
            registry.run_external(&mut app, "docker.close"),
            Err(crate::commands::ExternalError::NotExternal)
        );
    }

    #[test]
    fn a_stats_event_reaches_the_host_through_the_event_loop_path() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let containers_buffer = app.docker().session.as_ref().unwrap().containers_buffer;
        let status_buffer = app.docker().session.as_ref().unwrap().status_buffer;
        app.docker_mut().session.as_mut().unwrap().containers = vec![container("abc123", "web")];
        app.docker_mut().lines.insert(
            containers_buffer,
            vec![Some(docker_panel::DockerLine {
                style: docker_panel::DockerLineStyle::Container,
                entry: Some(docker_panel::DockerEntry::Container("abc123".to_string())),
                dim_from: None,
                badge: None,
            })],
        );
        app.test_set_cursor(Cursor {
            char_idx: 0,
            sticky_col: 0,
        });

        let stat = fenix_docker::ContainerStat {
            container_id: "abc123".to_string(),
            cpu_percent: "7%".to_string(),
            mem_usage: "1MiB".to_string(),
        };
        let event = DockerEvent::Stats(vec![stat]).into_user_event();
        assert_eq!(event.label(), "StatsReady");
        app.handle_user_event(event);

        assert!(app
            .buffers
            .get(status_buffer)
            .unwrap()
            .buffer
            .text()
            .contains("CPU: 7%"));
    }

    #[test]
    fn log_events_reach_the_logs_pane_through_the_event_loop_path() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let logs_buffer = app.docker().session.as_ref().unwrap().logs_buffer;
        app.handle_user_event(
            DockerEvent::LogLine(logs_buffer, "hello".to_string()).into_user_event(),
        );
        app.handle_user_event(DockerEvent::LogEnded(logs_buffer).into_user_event());
        let text = app.buffers.get(logs_buffer).unwrap().buffer.text();
        assert!(text.contains("hello\n-- log stream ended --"), "{text:?}");
    }

    #[test]
    fn pane_keys_are_claimed_only_in_the_sessions_panes() {
        let mut app = App::with_file(None);
        assert!(
            !app.feature_claims(|feature, app| feature.pane_key(app, KeyPress::char('u'))),
            "no session, nothing to claim"
        );

        app.open_docker_panel();
        let images_pane = app.docker().session.as_ref().unwrap().images_pane;
        assert!(app.feature_claims(|feature, app| feature.pane_key(app, KeyPress::char('2'))));
        assert_eq!(app.focused_pane_id(), images_pane);
        // Motions fall through to Vim.
        assert!(!app.feature_claims(|feature, app| feature.pane_key(app, KeyPress::char('j'))));

        app.new_workspace();
        assert!(
            !app.feature_claims(|feature, app| feature.pane_key(app, KeyPress::char('u'))),
            "a pane in another workspace"
        );
    }

    #[test]
    fn the_x_menu_shows_as_a_feature_popup_until_the_next_key() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        assert!(app.feature_claims(|feature, app| feature.pane_key(app, KeyPress::char('x'))));
        let popup = app
            .features
            .iter()
            .find_map(|feature| feature.popup(&app, 800.0, 580.0));
        assert_eq!(
            popup.map(|(id, ..)| id),
            Some(popup::PopupId::Feature("fenix.docker"))
        );
        app.each_feature(|feature, _| feature.key_arrived());
        assert!(app
            .features
            .iter()
            .all(|feature| feature.popup(&app, 800.0, 580.0).is_none()));
    }

    #[test]
    fn an_armed_remove_captures_exactly_one_key_and_shows_its_prompt() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        assert!(!app.feature_claims(|feature, app| feature.capture_key(app, KeyPress::char('n'))));
        app.docker_mut().confirm_remove =
            Some(docker_panel::DockerEntry::Network("net1".to_string()));
        assert_eq!(
            app.features
                .iter()
                .find_map(|feature| feature.prompt_text())
                .as_deref(),
            Some("Remove this network? (y/n)")
        );
        assert!(app.feature_claims(|feature, app| feature.capture_key(app, KeyPress::char('n'))));
        assert!(app.docker().confirm_remove.is_none());
        assert!(app
            .features
            .iter()
            .all(|feature| feature.prompt_text().is_none()));
        assert!(!app.feature_claims(|feature, app| feature.capture_key(app, KeyPress::char('n'))));
    }

    #[test]
    fn closing_a_buffer_closes_the_session_only_for_its_own_buffers() {
        let mut app = App::with_file(None);
        let scratch = app.buffers.open_scratch();
        app.open_docker_panel();
        assert!(!app.feature_claims(|feature, app| feature.close_buffer(app, scratch)));
        assert!(app.docker().session.is_some());
        let status_buffer = app.docker().session.as_ref().unwrap().status_buffer;
        assert!(app.feature_claims(|feature, app| feature.close_buffer(app, status_buffer)));
        assert!(app.docker().session.is_none());
        assert!(app.buffers.get(status_buffer).is_none());
    }

    #[test]
    fn closing_the_frame_that_holds_the_session_closes_it() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let workspaces = app.workspaces.len();
        app.each_feature(|feature, app| feature.close_panes(app, &[]));
        assert!(
            app.docker().session.is_some(),
            "no pane of the session is closing"
        );
        let panes = app.workspaces.all_panes();
        app.each_feature(|feature, app| feature.close_panes(app, &panes));
        assert!(app.docker().session.is_none());
        assert_eq!(app.workspaces.len(), workspaces - 1);
    }

    #[test]
    fn an_open_session_blocks_the_jumplist_and_guards_its_focused_pane() {
        let mut app = App::with_file(None);
        assert!(app.jumplist_navigation_allowed());
        assert!(!app.focused_pane_holds_a_tracked_session());
        app.open_docker_panel();
        assert!(!app.jumplist_navigation_allowed());
        assert!(app.focused_pane_holds_a_tracked_session());
        app.docker_session_close();
        assert!(app.jumplist_navigation_allowed());
    }

    #[test]
    fn only_docker_buffers_get_docker_highlights() {
        let mut app = App::with_file(None);
        let scratch = app.buffers.open_scratch();
        app.open_docker_panel();
        let containers_buffer = app.docker().session.as_ref().unwrap().containers_buffer;
        let theme = app.theme;
        let docker = app.docker();
        assert!(docker
            .highlights(app.buffers.get(scratch).unwrap(), scratch, 0, 10, theme)
            .is_none());
        assert!(docker
            .highlights(
                app.buffers.get(containers_buffer).unwrap(),
                containers_buffer,
                0,
                10,
                theme
            )
            .is_some());
    }
}
