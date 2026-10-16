//! Features: the in-process draft of the extension host API (Phase 0
//! of `docs/EXTENSIONS_PLAN.md`).
//!
//! A feature is everything one optional part of Fenix owns -- its state,
//! commands, keys, events and its share of the UI -- behind one trait,
//! instead of fields and `match` arms spread through `App`. `App` keeps
//! the instances in `Features` and calls each hook at the one place the
//! feature used to be wired in by hand, so behaviour doesn't change.
//!
//! Two halves:
//!
//! - `FeatureSpec` is static: the commands and leader keys a feature
//!   contributes. The command registry and the leader trie are built
//!   without an `App`, so these can't live on the instance.
//! - `Feature` is the instance: its hooks run with the feature taken out
//!   of `App` for the call (`App::with_feature`), so a hook gets both
//!   `&mut self` and `&mut App` without the two borrows meeting.
//!
//! Only the hooks a moved feature needs are here; each feature that
//! moves adds what it is missing. That list is what the WIT host API is
//! designed from.

use std::any::Any;

use super::docker_host::DockerHost;
use super::embedded_host::EmbeddedHost;
use super::*;
use crate::popup::PopupId;

/// A command a feature contributes. `run` gets the whole `App`, like
/// every other `fn(&mut App)` command handler.
pub(crate) struct FeatureCommand {
    pub id: &'static str,
    pub description: &'static str,
    /// May run from the control channel (`fenix/runCommand`).
    pub external: bool,
    pub run: fn(&mut App),
}

/// A leader-key entry, as the keys after `SPC`.
pub(crate) enum LeaderEntry {
    /// A which-key group label.
    Group(&'static [char], &'static str),
    /// A binding: keys, which-key label, command id.
    Bind(&'static [char], &'static str, &'static str),
}

/// What a feature contributes without an instance.
pub(crate) struct FeatureSpec {
    /// The id it will keep as an extension (`fenix.docker`).
    pub id: &'static str,
    pub commands: &'static [FeatureCommand],
    pub leader: &'static [LeaderEntry],
    /// Its `SPC m` menu, as the keys after `SPC m`, shown in the buffers
    /// `local_context` claims.
    pub local_menu: &'static [LeaderEntry],
    /// Whether its `SPC m` menu applies to the focused buffer, and under
    /// which context.
    pub local_context: Option<fn(&App) -> Option<crate::keymap::LocalContext>>,
    /// The language server for `language` files under `root`, when this
    /// feature knows better than the defaults. `Some(Err)` says why it
    /// can't start one.
    pub language_server: Option<LanguageServerFn>,
    /// Tasks it offers in the task picker for the project at `root`.
    pub tasks: Option<fn(&App, &Path) -> Vec<fenix_tasks::TaskDef>>,
}

/// See `FeatureSpec::language_server`.
pub(crate) type LanguageServerFn = fn(
    &App,
    &Path,
    fenix_syntax::LanguageId,
) -> Option<Result<fenix_project::tools::CommandSpec, String>>;

// The queries above live on the spec, not the instance, because they need
// no state -- and a feature's instance is out of `App` while one of its own
// hooks runs. A hook that opens a file or restarts a language server would
// otherwise ask every feature but itself.

/// Something that happened that features may want to follow up on. One
/// that arrives while a feature is out (inside one of its own hooks) is
/// kept and delivered when the feature is put back, so a feature hears
/// about what its own hooks caused.
#[derive(Debug, Clone)]
pub(crate) enum Notice {
    /// The focused buffer changed.
    FocusChanged,
    /// A task in the project at `root` finished.
    TaskFinished { root: PathBuf, success: bool },
    /// A terminal buffer was closed.
    TerminalClosed(BufferId),
}

/// One row's payload in a feature's picker (`ActivePicker::Feature`):
/// whatever the feature put there, handed back to its `picker_confirm`.
#[derive(Clone)]
pub(crate) struct FeaturePick(pub(crate) Rc<dyn Any>);

impl FeaturePick {
    pub(crate) fn candidate<T: Any>(
        label: impl Into<String>,
        payload: T,
    ) -> fenix_picker::Candidate<FeaturePick> {
        fenix_picker::Candidate::new(label, FeaturePick(Rc::new(payload)))
    }
}

/// A message from one of a feature's background threads, delivered to
/// its `on_event` on the UI thread.
pub struct FeatureEvent {
    pub feature: &'static str,
    /// What kind of event it is, for the hang log.
    pub label: &'static str,
    pub payload: Box<dyn Any + Send>,
}

impl std::fmt::Debug for FeatureEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeatureEvent")
            .field("feature", &self.feature)
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

/// Text colour ranges for the visible rows of a buffer a feature draws.
pub(crate) type Highlights = Vec<(std::ops::Range<usize>, glyphon::Color)>;

/// A feature's live half. Every hook has a default that does nothing,
/// so a feature implements only what it uses.
pub(crate) trait Feature: Any {
    fn spec(&self) -> &'static FeatureSpec;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// A `FeatureEvent` addressed to this feature arrived.
    fn on_event(&mut self, _app: &mut App, _payload: Box<dyn Any + Send>) {}

    /// A key arrived, before anything handles it: drop transient UI
    /// (an informational menu) that any key dismisses.
    fn key_arrived(&mut self) {}
    /// A prompt of this feature's is waiting for exactly one key.
    /// `true` when it took the key.
    fn capture_key(&mut self, _app: &mut App, _key: KeyPress) -> bool {
        false
    }
    /// A key in one of this feature's panes, before Vim sees it. `true`
    /// when the feature handled it.
    fn pane_key(&mut self, _app: &mut App, _key: KeyPress) -> bool {
        false
    }
    /// Vim handled a key (a motion, a search) and the cursor may have
    /// moved.
    fn after_vim_key(&mut self, _app: &mut App) {}

    /// The focused pane belongs to this feature, so opening a file
    /// mustn't replace what it shows.
    fn holds_focused_pane(&self, _app: &App) -> bool {
        false
    }
    /// Jumplist navigation would desync this feature's panes.
    fn blocks_jumplist(&self) -> bool {
        false
    }
    /// The frame holding `panes` is closing.
    fn close_panes(&mut self, _app: &mut App, _panes: &[fenix_window::WindowId]) {}
    /// `SPC b k` on `buffer`. `true` when the feature closed it its own
    /// way and the generic close mustn't run.
    fn close_buffer(&mut self, _app: &mut App, _buffer: BufferId) -> bool {
        false
    }

    /// The modeline's prompt text while this feature asks something.
    fn prompt_text(&self) -> Option<String> {
        None
    }
    /// Extra modeline text after the file name (Arduino's board and port).
    fn modeline_indicator(&self) -> Option<&str> {
        None
    }
    /// A `Notice`.
    fn notice(&mut self, _app: &mut App, _notice: &Notice) {}

    /// The title text of this feature's open picker (`ActivePicker::
    /// Feature`), what it's choosing.
    fn picker_label(&self) -> Option<&str> {
        None
    }
    /// A row of this feature's picker was chosen.
    fn picker_confirm(&mut self, _app: &mut App, _pick: &dyn Any) {}
    /// This feature's picker was cancelled.
    fn picker_cancel(&mut self) {}

    /// Colours for `buffer`'s visible rows, when this feature draws it.
    /// Given the theme rather than `App`: the caller holds the buffer
    /// mutably while it asks.
    fn highlights(
        &self,
        _ob: &OpenBuffer,
        _buffer: BufferId,
        _first_row: usize,
        _rows: usize,
        _theme: &Theme,
    ) -> Option<Highlights> {
        None
    }
    /// A popup to show this frame.
    fn popup(
        &self,
        _app: &App,
        _window_width: f32,
        _modeline_top: f32,
    ) -> Option<(PopupId, fenix_window::Rect, RowSpans)> {
        None
    }
}

/// The built-in features. A slot is empty only while its feature is
/// out for a hook (`App::with_feature_at`).
pub(crate) struct Features {
    slots: Vec<Option<Box<dyn Feature>>>,
    /// Each slot's spec, there even while the feature is out.
    specs: Vec<&'static FeatureSpec>,
    /// Notices a feature missed while it was out, by slot.
    missed: Vec<(usize, Notice)>,
}

impl Features {
    pub(crate) fn builtin() -> Self {
        let slots: Vec<Option<Box<dyn Feature>>> = vec![
            Some(Box::new(DockerHost::default())),
            Some(Box::new(EmbeddedHost::default())),
        ];
        let specs = slots
            .iter()
            .map(|slot| slot.as_deref().expect("all in place").spec())
            .collect();
        Self {
            slots,
            specs,
            missed: Vec::new(),
        }
    }

    /// Every feature's spec, whether or not it's in place.
    pub(crate) fn specs(&self) -> impl Iterator<Item = &'static FeatureSpec> + '_ {
        self.specs.iter().copied()
    }

    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    /// Every feature that's in place.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &dyn Feature> {
        self.slots.iter().filter_map(|slot| slot.as_deref())
    }

    /// Typed access. Only tests need it so far: everything else goes
    /// through the hooks or `App::with_feature`.
    #[cfg(test)]
    pub(crate) fn get<T: Feature>(&self) -> Option<&T> {
        self.iter().find_map(|f| f.as_any().downcast_ref::<T>())
    }

    #[cfg(test)]
    pub(crate) fn get_mut<T: Feature>(&mut self) -> Option<&mut T> {
        self.slots
            .iter_mut()
            .filter_map(|slot| slot.as_deref_mut())
            .find_map(|f| f.as_any_mut().downcast_mut::<T>())
    }

    fn position_of<T: Feature>(&self) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| slot.as_deref().is_some_and(|f| f.as_any().is::<T>()))
    }

    /// The feature whose id is `id`, if it's in place.
    pub(crate) fn by_id(&self, id: &str) -> Option<&dyn Feature> {
        self.iter().find(|f| f.spec().id == id)
    }

    fn position_of_id(&self, id: &str) -> Option<usize> {
        self.specs.iter().position(|spec| spec.id == id)
    }
}

impl App {
    /// Runs `f` with feature `i` taken out of `App`, then puts it back.
    /// `None` when the slot is empty (the feature is already out, in a
    /// hook that came back into `App`).
    fn with_feature_at<R>(
        &mut self,
        i: usize,
        f: impl FnOnce(&mut dyn Feature, &mut App) -> R,
    ) -> Option<R> {
        let mut feature = self.features.slots.get_mut(i)?.take()?;
        let result = f(feature.as_mut(), self);
        self.put_back(i, feature);
        Some(result)
    }

    /// Puts feature `i` back, then delivers the notices it missed while it
    /// was out -- including any those cause.
    fn put_back(&mut self, i: usize, feature: Box<dyn Feature>) {
        self.features.slots[i] = Some(feature);
        while let Some(at) = self.features.missed.iter().position(|(slot, _)| *slot == i) {
            let (_, notice) = self.features.missed.remove(at);
            let mut feature = self.features.slots[i].take().expect("just put back");
            feature.notice(self, &notice);
            self.features.slots[i] = Some(feature);
        }
    }

    /// Runs `f` with the `T` feature and the rest of `App`.
    pub(crate) fn with_feature<T: Feature, R>(
        &mut self,
        f: impl FnOnce(&mut T, &mut App) -> R,
    ) -> Option<R> {
        let i = self.features.position_of::<T>()?;
        self.with_feature_at(i, |feature, app| {
            let feature = feature
                .as_any_mut()
                .downcast_mut::<T>()
                .expect("the slot was found by its type");
            f(feature, app)
        })
    }

    /// Offers `f` to each feature in turn until one returns `true`.
    pub(crate) fn feature_claims(
        &mut self,
        mut f: impl FnMut(&mut dyn Feature, &mut App) -> bool,
    ) -> bool {
        (0..self.features.len()).any(|i| self.with_feature_at(i, &mut f).unwrap_or(false))
    }

    /// Runs `f` on every feature.
    pub(crate) fn each_feature(&mut self, mut f: impl FnMut(&mut dyn Feature, &mut App)) {
        for i in 0..self.features.len() {
            self.with_feature_at(i, &mut f);
        }
    }

    /// Tells every feature about `notice` -- now, or, for one that's out,
    /// as soon as it's back.
    pub(crate) fn notify_features(&mut self, notice: Notice) {
        for i in 0..self.features.len() {
            if self.features.slots[i].is_some() {
                self.with_feature_at(i, |feature, app| feature.notice(app, &notice));
            } else {
                self.features.missed.push((i, notice.clone()));
            }
        }
    }

    /// Delivers a background event to the feature it's addressed to.
    pub(crate) fn deliver_feature_event(&mut self, event: FeatureEvent) {
        self.with_feature_id(event.feature, |feature, app| {
            feature.on_event(app, event.payload)
        });
    }

    /// Runs `f` with the feature whose id is `id`.
    pub(crate) fn with_feature_id<R>(
        &mut self,
        id: &str,
        f: impl FnOnce(&mut dyn Feature, &mut App) -> R,
    ) -> Option<R> {
        let i = self.features.position_of_id(id)?;
        self.with_feature_at(i, f)
    }
}
