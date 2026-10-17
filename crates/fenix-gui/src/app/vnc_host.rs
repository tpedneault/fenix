//! VNC as a feature (`fenix.vnc`): its commands, `SPC v`, the host
//! picker, and the state its code reaches by type. That code is still
//! `App` methods -- a VNC pane is drawn by core, its framebuffer through
//! the core surface pipeline (`crate::surface`).

use super::*;

/// VNC's commands, its `SPC v` group and its host picker.
pub(crate) static SPEC: super::feature::FeatureSpec = {
    use super::feature::FeatureCommand as C;
    use super::feature::LeaderEntry::{Bind, Group};
    super::feature::FeatureSpec {
        id: "fenix.vnc",
        commands: &[
            C { id: "vnc.open", description: "Open or switch to a configured VNC session", external: true, run: |app| app.start_vnc_picker() },
            C { id: "vnc.close", description: "Close the focused VNC session", external: false, run: |app| app.vnc_close_focused_session() },
            C { id: "vnc.screenshot", description: "Save the focused VNC session's current frame as a PNG", external: false, run: |app| app.vnc_screenshot() },
        ],
        leader: &[
            Group(&['v'], "vnc"),
            Bind(&['v', 'v'], "open/switch vnc session", "vnc.open"),
            Bind(&['v', 'q'], "close vnc session", "vnc.close"),
            Bind(&['v', 's'], "save vnc screenshot", "vnc.screenshot"),
        ],
        local_menu: &[],
        local_context: None,
        language_server: None,
        tasks: None,
        hover_fallback: None,
        completion_at_cursor: None,
        completion_pool_at: None,
        hover_first: None,
        // A configured host, by name: connects the first time it's opened
        // this run, otherwise switches to the already-live session.
        picker_confirm: Some(|app, pick, _marked, _query| {
            if let Some(name) = pick.downcast_ref::<String>() {
                app.open_vnc_session(name);
            }
        }),
        picker_cancel: None,
        modeline_clock: None,
        page_action: None,
        page_context: None,
        page_title: None,
        page_after_layout: None,
        on_event: Some(|app, event| match event.downcast::<VncEvent>().map(|e| *e) {
            Ok(VncEvent::Connected { name, host, port, result }) => app.apply_vnc_connected(name, host, port, result.0),
            Ok(VncEvent::Frame(key, frame)) => app.apply_vnc_frame(key, frame),
            Err(_) => {}
        }),
    }
};

/// What VNC's background threads send: a connection made (or not), and
/// each decoded update.
pub(crate) enum VncEvent {
    /// The result of `fenix_vnc::VncClient::connect`, from the one-shot
    /// thread `open_vnc_session`/`schedule_vnc_reconnect` spawns it on (a
    /// slow or unreachable VM would otherwise freeze the editor). `name`
    /// is the VM's configured name; `host`/`port` ride along so a
    /// reconnect completes even if the settings changed meanwhile.
    Connected { name: String, host: String, port: u16, result: VncConnectResult },
    /// One decoded update, from a session's reader thread, for VM `name`.
    Frame(String, fenix_vnc::VncFrame),
}

/// VNC's state: the live sessions, which one has the keyboard, those
/// still connecting, and what drawing them needs. Its code is still `App`
/// methods, so it reaches this by type (`Features::vnc`) and has no
/// instance hooks that run code.
#[derive(Default)]
pub(crate) struct VncState {
    /// Every currently-open VNC session (`SPC v v`), keyed by the VM's
    /// configured name (`Config.vnc_hosts`) -- a `HashMap`, not a single
    /// `Option<VncSession>` like `DockerHost::session`/`git_session`/
    /// `jira_session`: those are each a singleton tool where a second
    /// concurrent session has no meaning, while several simultaneous,
    /// independently-live VM consoles is the entire point here.
    pub(super) sessions: HashMap<String, VncSession>,
    /// Reusable staging buffer for VNC texture uploads -- `redraw`
    /// extracts each session's dirty sub-rectangle into this before
    /// handing it to the GPU (the framebuffer itself is strided by its
    /// full width, so a sub-rect can't be uploaded in place). Held here,
    /// rather than allocated per frame, because a full-screen dirty
    /// region runs to several megabytes.
    pub(super) upload_scratch: Vec<u8>,
    /// The VM name whose session currently owns raw keyboard input, if
    /// any -- same "this pane eats every key, Vim doesn't apply" role as
    /// `terminal_focused`, generalized to name *which* of up to several
    /// concurrent sessions it refers to (there's still only ever one
    /// focused at a time). `Ctrl-\` clears it, same chord/meaning as
    /// `terminal_focused`'s own.
    pub(super) focused: Option<String>,
    /// A fully transparent 1x1 cursor image, created once against the
    /// first frame's `ActiveEventLoop` (`resumed`) and reused for every
    /// window/session after that -- `set_vnc_focused`'s way of hiding
    /// the host cursor while a VNC session has capture. See that
    /// field's own doc comment for why this exists instead of the
    /// obvious `Window::set_cursor_visible(false)`. `None` until the
    /// first `resumed`, same posture as `gpu`/`text`/every other
    /// GPU-loop-owned resource.
    pub(super) hidden_cursor: Option<winit::window::CustomCursor>,
    /// VM names with a connect currently in flight on a background
    /// thread -- lets `open_vnc_session` show a "Connecting..."
    /// placeholder and refuse a redundant second connect attempt if the
    /// picker is used again on the same name before the first finishes.
    pub(super) connecting: HashSet<String>,
}

#[derive(Default)]
pub(crate) struct VncHost {
    pub(super) state: VncState,
}

impl super::feature::Feature for VncHost {
    fn spec(&self) -> &'static super::feature::FeatureSpec {
        &SPEC
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl super::feature::Features {
    pub(super) fn vnc(&self) -> &VncState {
        &self.get::<VncHost>().expect("VNC has no instance hooks that run code, so it's always in place").state
    }

    pub(super) fn vnc_mut(&mut self) -> &mut VncState {
        &mut self.get_mut::<VncHost>().expect("VNC has no instance hooks that run code, so it's always in place").state
    }
}

/// The `SPC v v` picker over configured host names.
pub(super) fn vnc_picker(candidates: Vec<fenix_picker::Candidate<String>>) -> ActivePicker {
    let candidates = candidates.into_iter().map(|c| super::feature::FeaturePick::candidate(c.label, c.payload)).collect();
    ActivePicker::Feature { feature: SPEC.id, badge: "VNC", state: fenix_picker::PickerState::new(candidates) }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Through its spec, the way `App` reaches VNC now.

    #[test]
    fn vnc_contributes_its_commands_and_keys_from_its_spec() {
        let registry = CommandRegistry::with_builtins();
        let vnc: Vec<(&str, bool)> = registry.iter().filter(|c| c.id.starts_with("vnc.")).map(|c| (c.id.as_ref(), c.external())).collect();
        assert_eq!(vnc, [("vnc.open", true), ("vnc.close", false), ("vnc.screenshot", false)]);
        let keys = keymap::leader_keys();
        assert_eq!(keys.get("vnc.open").map(String::as_str), Some("SPC v v"));
        assert_eq!(keys.get("vnc.screenshot").map(String::as_str), Some("SPC v s"));
    }

    #[test]
    fn nothing_is_open_or_connecting_at_startup() {
        let app = App::with_file(None);
        let vnc = app.features.vnc();
        assert!(vnc.sessions.is_empty() && vnc.connecting.is_empty() && vnc.focused.is_none());
    }
}
