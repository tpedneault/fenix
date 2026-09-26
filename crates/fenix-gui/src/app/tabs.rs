//! A pane's tabs: the workspace's Home pinned first, then the buffers
//! the pane lists in `pane_tabs` -- and the keys that move along them
//! (`gt`, `gT`, `{n}gt`, `g<Tab>`, `gh`, `Ctrl-PgDn`/`Ctrl-PgUp`/
//! `Ctrl-Tab`). The order is the same whether or not the theme draws a
//! strip, so the keys work in every theme.

use super::*;
use fenix_vim::TabMove;

/// Where `mv` lands in `order` (Home first, then the pane's tabs) from
/// `current`. `None` when there's nowhere to go: an empty order, or a
/// `{n}gt` past the last tab. `Last` and `Home` aren't positional and
/// are the caller's to resolve.
pub(super) fn step(order: &[BufferId], current: Option<BufferId>, mv: TabMove) -> Option<BufferId> {
    let len = order.len();
    if len == 0 {
        return None;
    }
    let at = current.and_then(|c| order.iter().position(|&id| id == c));
    match mv {
        TabMove::Next => Some(order[at.map_or(1 % len, |i| (i + 1) % len)]),
        TabMove::Prev(n) => {
            let i = at.unwrap_or(0);
            let back = n as usize % len;
            Some(order[(i + len - back) % len])
        }
        // Home sits at 0 and isn't numbered, so tab `n` is index `n`.
        TabMove::Nth(n) => order.get(n as usize).copied(),
        TabMove::Last | TabMove::Home => None,
    }
}

/// The tab to show once `closed` goes: the one to its right, or at the
/// end of the strip the one to its left (Visual Studio's rule).
/// `order` is the strip before closing. `None` if `closed` was the only
/// tab.
pub(super) fn tab_after_closing(order: &[BufferId], closed: BufferId) -> Option<BufferId> {
    let i = order.iter().position(|&id| id == closed)?;
    order.get(i + 1).or_else(|| i.checked_sub(1).and_then(|j| order.get(j))).copied()
}

impl App {
    /// Every workspace, in every frame -- the live one's and the parked
    /// ones'.
    fn all_workspaces(&self) -> impl Iterator<Item = &Workspace> {
        let parked = self.frames.iter().flatten().map(|frame| &frame.workspaces);
        std::iter::once(&self.workspaces).chain(parked).flat_map(|list| list.workspaces.iter())
    }

    /// Whether `id` is some workspace's Home.
    pub(super) fn is_a_workspace_home(&self, id: BufferId) -> bool {
        self.all_workspaces().any(|ws| ws.home == Some(id))
    }

    /// The project a Home buffer is scoped to: its workspace's.
    pub(super) fn home_project_of(&self, id: BufferId) -> Option<PathBuf> {
        self.all_workspaces().find(|ws| ws.home == Some(id)).and_then(|ws| ws.project.clone())
    }

    /// Every project some workspace's Home is scoped to.
    pub(super) fn home_projects(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = self.all_workspaces().filter(|ws| ws.home.is_some()).filter_map(|ws| ws.project.clone()).collect();
        roots.sort();
        roots.dedup();
        roots
    }

    /// `pane`'s tabs in strip order: the workspace's Home, then the
    /// buffers `pane_tabs` lists that are still open.
    pub(super) fn pane_tab_order(&self, pane: fenix_window::WindowId) -> Vec<BufferId> {
        let ws = self.workspaces.active_workspace();
        let home = ws.home.filter(|id| self.buffers.get(*id).is_some());
        let files = ws.pane_tabs.get(&pane).into_iter().flatten().copied();
        home.into_iter().chain(files.filter(|&id| Some(id) != home && self.buffers.get(id).is_some())).collect()
    }

    /// The label a tab shows: the workspace's name on its Home, the
    /// buffer's name everywhere else.
    pub(super) fn tab_label(&self, id: BufferId) -> String {
        let ws = self.workspaces.active_workspace();
        if ws.home == Some(id) {
            ws.name.clone()
        } else {
            self.buffer_display_name(id)
        }
    }

    /// The active workspace's Home, made if it has none yet: a Home
    /// already in it (the start screen, a new workspace's first buffer)
    /// is adopted, unless another workspace owns it.
    pub(super) fn ensure_workspace_home(&mut self) -> BufferId {
        let ws = self.workspaces.active_workspace();
        if let Some(id) = ws.home.filter(|id| self.buffers.get(*id).is_some()) {
            return id;
        }
        let shown = ws.windows.windows().into_iter().filter_map(|pane| ws.windows.content(pane).copied());
        let listed = ws.pane_tabs.values().flatten().copied();
        let shown: Vec<BufferId> = shown.chain(listed).collect();
        let adopt = shown
            .into_iter()
            .find(|&id| self.buffers.get(id).is_some_and(|ob| ob.kind == BufferKind::Dashboard) && !self.is_a_workspace_home(id));
        let id = adopt.unwrap_or_else(|| self.buffers.open_dashboard(""));
        let ws = self.workspaces.active_workspace_mut();
        ws.home = Some(id);
        for tabs in ws.pane_tabs.values_mut() {
            tabs.retain(|&b| b != id);
        }
        self.refresh_home_data(true);
        id
    }

    /// `gh`/`SPC o d`: the workspace's Home, in the focused pane.
    pub(crate) fn go_home(&mut self) {
        let home = self.ensure_workspace_home();
        if self.focused_buffer_id() != home {
            self.open_buffer_in_focused_pane(home);
            self.refresh_project_root();
        }
        self.wake_caret();
    }

    /// Moves the focused pane along its tabs. A fixed panel (a titled
    /// Git, Docker or terminal pane) has no tabs, so it stays put.
    pub(super) fn move_tab(&mut self, mv: TabMove) {
        let pane = self.focused_pane_id();
        if self.pane_titles.contains_key(&pane) {
            return;
        }
        if mv == TabMove::Home {
            self.go_home();
            return;
        }
        self.ensure_workspace_home();
        let order = self.pane_tab_order(pane);
        let current = self.windows().content(pane).copied();
        let target = match mv {
            TabMove::Last => self.workspaces.active_workspace().last_tab.get(&pane).copied().filter(|id| order.contains(id)),
            _ => step(&order, current, mv),
        };
        match target {
            Some(target) if Some(target) != current => {
                self.set_pane_content(pane, target);
                self.refresh_project_root();
            }
            Some(_) => {}
            None => {
                if let TabMove::Nth(n) = mv {
                    self.set_message(format!("no tab {n}"));
                }
            }
        }
        self.wake_caret();
    }

    /// After `id` is closed for good: gone from every pane's tabs in
    /// every workspace, and each pane that was showing it moves to its
    /// neighbour tab, or to its workspace's Home when it had no other.
    pub(super) fn drop_buffer_from_every_strip(&mut self, id: BufferId) {
        // Worked out before touching anything: which tab each affected
        // pane goes to, per (list, workspace, pane).
        let mut moves = Vec::new();
        for (list_index, ws_index, ws) in self.workspace_lists().flat_map(|(l, list)| list.workspaces.iter().enumerate().map(move |(w, ws)| (l, w, ws))) {
            for pane in ws.windows.windows() {
                if ws.windows.content(pane) != Some(&id) {
                    continue;
                }
                let home = ws.home.filter(|h| self.buffers.get(*h).is_some());
                let order: Vec<BufferId> = home
                    .into_iter()
                    .chain(ws.pane_tabs.get(&pane).into_iter().flatten().copied().filter(|&b| b == id || (Some(b) != home && self.buffers.get(b).is_some())))
                    .collect();
                moves.push((list_index, ws_index, pane, tab_after_closing(&order, id)));
            }
        }
        let active = (0, self.workspaces.active_index());
        let mut active_panes = Vec::new();
        for (list_index, ws_index, pane, target) in moves {
            let target = match target {
                Some(target) => target,
                None => self.home_of(list_index, ws_index),
            };
            let cursor = self.buffers.get(target).map(|ob| ob.cursor).unwrap_or(Cursor::at_start());
            let Some(ws) = self.workspace_at_mut(list_index, ws_index) else { continue };
            ws.windows.set_content(pane, target);
            ws.pane_states.insert(pane, PaneState::seeded_at(cursor));
            if Some(target) != ws.home && !ws.pane_tabs.get(&pane).is_some_and(|t| t.contains(&target)) {
                ws.pane_tabs.entry(pane).or_default().push(target);
            }
            if (list_index, ws_index) == active {
                active_panes.push(pane);
            }
            self.buffers.touch(target);
            self.refresh_gutter_hunks(target);
        }
        for list in std::iter::once(&mut self.workspaces).chain(self.frames.iter_mut().flatten().map(|f| &mut f.workspaces)) {
            for ws in &mut list.workspaces {
                ws.pane_tabs.values_mut().for_each(|tabs| tabs.retain(|&b| b != id));
                ws.last_tab.retain(|_, b| *b != id);
            }
        }
        if self.snippet.as_ref().is_some_and(|s| active_panes.contains(&s.pane)) {
            self.snippet = None;
        }
    }

    /// Every workspace list with its index: 0 is the live frame's, the
    /// rest are parked frames' in order.
    fn workspace_lists(&self) -> impl Iterator<Item = (usize, &WorkspaceList)> {
        std::iter::once(&self.workspaces).chain(self.frames.iter().flatten().map(|frame| &frame.workspaces)).enumerate()
    }

    fn workspace_at_mut(&mut self, list_index: usize, ws_index: usize) -> Option<&mut Workspace> {
        let list = if list_index == 0 {
            &mut self.workspaces
        } else {
            &mut self.frames.iter_mut().flatten().nth(list_index - 1)?.workspaces
        };
        list.workspaces.get_mut(ws_index)
    }

    /// Workspace (`list_index`, `ws_index`)'s Home, made if it has none.
    fn home_of(&mut self, list_index: usize, ws_index: usize) -> BufferId {
        let existing = self
            .workspace_lists()
            .nth(list_index)
            .and_then(|(_, list)| list.workspaces.get(ws_index))
            .and_then(|ws| ws.home)
            .filter(|id| self.buffers.get(*id).is_some());
        if let Some(id) = existing {
            return id;
        }
        let id = self.buffers.open_dashboard("");
        if let Some(ws) = self.workspace_at_mut(list_index, ws_index) {
            ws.home = Some(id);
        }
        self.refresh_home_data(true);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<BufferId> {
        let mut buffers = fenix_buffers::BufferList::new();
        (0..n).map(|_| buffers.open_scratch()).collect()
    }

    #[test]
    fn next_and_previous_wrap_round_through_home() {
        let order = ids(4); // Home, then three files
        assert_eq!(step(&order, Some(order[1]), TabMove::Next), Some(order[2]));
        assert_eq!(step(&order, Some(order[3]), TabMove::Next), Some(order[0]), "gt from the last tab is Home");
        assert_eq!(step(&order, Some(order[0]), TabMove::Prev(1)), Some(order[3]), "gT from Home is the last tab");
        assert_eq!(step(&order, Some(order[3]), TabMove::Prev(2)), Some(order[1]));
    }

    #[test]
    fn a_counted_gt_counts_files_not_home() {
        let order = ids(4);
        assert_eq!(step(&order, Some(order[3]), TabMove::Nth(1)), Some(order[1]));
        assert_eq!(step(&order, Some(order[0]), TabMove::Nth(3)), Some(order[3]));
        assert_eq!(step(&order, Some(order[0]), TabMove::Nth(4)), None);
    }

    #[test]
    fn a_pane_showing_something_off_its_strip_starts_from_home() {
        let all = ids(4);
        let (order, stray) = (&all[..3], all[3]);
        assert_eq!(step(order, Some(stray), TabMove::Next), Some(order[1]));
    }

    #[test]
    fn closing_goes_right_then_left() {
        let order = ids(4);
        assert_eq!(tab_after_closing(&order, order[2]), Some(order[3]));
        assert_eq!(tab_after_closing(&order, order[3]), Some(order[2]));
        assert_eq!(tab_after_closing(&order[..2], order[1]), Some(order[0]), "the last file closes to Home");
        assert_eq!(tab_after_closing(&order[..1], order[0]), None);
    }
}
