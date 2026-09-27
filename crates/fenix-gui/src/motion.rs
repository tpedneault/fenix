//! What moves, and how far along it is. Settings decide which
//! animations play (`on`) and how long they take (`duration`); `Anim`
//! is one of them in flight. Everything takes the time from its caller,
//! so a test can ask where an animation is at any moment without
//! waiting for it.
//!
//! The rules every animation here keeps: a key acts at once and the
//! picture only catches up; nothing lasts long; frames are drawn only
//! while something moves.

use std::time::{Duration, Instant};

use fenix_config::{Config, MotionSettings};

/// How much moves, from `motion.level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Off,
    Subtle,
    Full,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Off => "off",
            Level::Subtle => "subtle",
            Level::Full => "full",
        }
    }

    pub fn parse(text: &str) -> Option<Level> {
        match text {
            "off" => Some(Level::Off),
            "subtle" => Some(Level::Subtle),
            "full" => Some(Level::Full),
            _ => None,
        }
    }

    /// What `SPC t a` goes to next.
    pub fn next(self) -> Level {
        match self {
            Level::Off => Level::Subtle,
            Level::Subtle => Level::Full,
            Level::Full => Level::Off,
        }
    }
}

/// One animation that can be turned on or off on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    CaretFade,
    SmoothScroll,
    YankPulse,
    Beacon,
    BeaconOnFocus,
    ChangePulse,
    Popups,
    Messages,
    ErrorFlash,
    Progress,
    ModeFade,
    Tabs,
    CaretGlide,
    Folds,
    Layout,
    ThemeFade,
    Splash,
}

impl Feature {
    /// The level that turns it on when it isn't set on its own.
    fn needs(self) -> Level {
        match self {
            Feature::CaretGlide | Feature::Folds | Feature::Layout => Level::Full,
            _ => Level::Subtle,
        }
    }

    fn setting(self, m: &MotionSettings) -> Option<bool> {
        match self {
            Feature::CaretFade => m.caret_fade,
            Feature::SmoothScroll => m.smooth_scroll,
            Feature::YankPulse => m.yank_pulse,
            Feature::Beacon => m.beacon,
            Feature::BeaconOnFocus => m.beacon_on_focus,
            Feature::ChangePulse => m.change_pulse,
            Feature::Popups => m.popups,
            Feature::Messages => m.messages,
            Feature::ErrorFlash => m.error_flash,
            Feature::Progress => m.progress,
            Feature::ModeFade => m.mode_fade,
            Feature::Tabs => m.tabs,
            Feature::CaretGlide => m.caret_glide,
            Feature::Folds => m.folds,
            Feature::Layout => m.layout,
            Feature::ThemeFade => m.theme_fade,
            Feature::Splash => m.splash,
        }
    }
}

/// The level in effect: `editor.animations = false` is off whatever
/// `motion.level` says.
pub fn level(config: &Config) -> Level {
    if config.animations == Some(false) {
        return Level::Off;
    }
    config.motion.level.as_deref().and_then(Level::parse).unwrap_or(Level::Subtle)
}

/// Whether `feature` plays: its own setting when it has one, else
/// whether the level includes it. `editor.animations = false` wins
/// over both.
pub fn on(config: &Config, feature: Feature) -> bool {
    if config.animations == Some(false) {
        return false;
    }
    feature.setting(&config.motion).unwrap_or_else(|| level(config) >= feature.needs())
}

/// `base_ms` scaled by `motion.speed`.
pub fn duration(config: &Config, base_ms: u64) -> Duration {
    let speed = config.motion.speed.filter(|s| s.is_finite() && *s > 0.0).unwrap_or(1.0);
    Duration::from_secs_f32(base_ms as f32 / 1000.0 * speed)
}

/// Every feature's switch and length, resolved from the settings once
/// -- what a frame reads, so drawing never has to reach for `Config`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prefs {
    pub caret_fade: bool,
    pub smooth_scroll: bool,
    pub yank_pulse: bool,
    pub beacon: bool,
    pub beacon_on_focus: bool,
    pub change_pulse: bool,
    pub popups: bool,
    pub messages: bool,
    pub error_flash: bool,
    pub progress: bool,
    pub mode_fade: bool,
    pub tabs: bool,
    pub caret_glide: bool,
    pub folds: bool,
    pub layout: bool,
    pub theme_fade: bool,
    pub splash: bool,
    pub blink_fade_d: Duration,
    pub scroll_d: Duration,
    pub pulse_d: Duration,
    pub beacon_d: Duration,
    pub popup_in_d: Duration,
    pub popup_out_d: Duration,
    pub message_in_d: Duration,
    pub message_out_d: Duration,
    pub error_d: Duration,
    pub mode_d: Duration,
    pub tab_d: Duration,
    pub glide_d: Duration,
    pub fold_d: Duration,
    pub layout_d: Duration,
    pub theme_d: Duration,
}

impl Prefs {
    pub fn from_config(c: &Config) -> Prefs {
        let m = &c.motion;
        Prefs {
            caret_fade: on(c, Feature::CaretFade),
            smooth_scroll: on(c, Feature::SmoothScroll),
            yank_pulse: on(c, Feature::YankPulse),
            beacon: on(c, Feature::Beacon),
            beacon_on_focus: on(c, Feature::Beacon) && on(c, Feature::BeaconOnFocus),
            change_pulse: on(c, Feature::ChangePulse),
            popups: on(c, Feature::Popups),
            messages: on(c, Feature::Messages),
            error_flash: on(c, Feature::ErrorFlash),
            progress: on(c, Feature::Progress),
            mode_fade: on(c, Feature::ModeFade),
            tabs: on(c, Feature::Tabs),
            caret_glide: on(c, Feature::CaretGlide),
            folds: on(c, Feature::Folds),
            layout: on(c, Feature::Layout),
            theme_fade: on(c, Feature::ThemeFade),
            splash: on(c, Feature::Splash),
            blink_fade_d: duration(c, 120),
            scroll_d: duration(c, m.scroll_ms.unwrap_or(150)),
            pulse_d: duration(c, 300),
            beacon_d: duration(c, m.beacon_ms.unwrap_or(300)),
            popup_in_d: duration(c, 90),
            popup_out_d: duration(c, 60),
            message_in_d: duration(c, 120),
            message_out_d: duration(c, 300),
            error_d: duration(c, 450),
            mode_d: duration(c, 80),
            tab_d: duration(c, 80),
            glide_d: duration(c, m.glide_ms.unwrap_or(45)),
            fold_d: duration(c, 70),
            layout_d: duration(c, 80),
            theme_d: duration(c, 200),
        }
    }
}

/// One animation in flight: when it started and how long it lasts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anim {
    pub started: Instant,
    pub duration: Duration,
}

impl Anim {
    pub fn new(now: Instant, duration: Duration) -> Self {
        Anim { started: now, duration }
    }

    /// 0 at the start, 1 at the end, linear.
    pub fn linear(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        (now.saturating_duration_since(self.started).as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0)
    }

    /// 0 at the start, 1 at the end, fast then settling.
    pub fn eased(&self, now: Instant) -> f32 {
        ease_out_cubic(self.linear(now))
    }

    pub fn running(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) < self.duration
    }
}

/// Fast at the start, settling at the end -- every animation's curve.
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn lerp_rgba(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t), lerp(a[3], b[3], t)]
}

pub fn lerp_rect(a: fenix_window::Rect, b: fenix_window::Rect, t: f32) -> fenix_window::Rect {
    fenix_window::Rect { x: lerp(a.x, b.x, t), y: lerp(a.y, b.y, t), w: lerp(a.w, b.w, t), h: lerp(a.h, b.h, t) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config::empty(std::path::PathBuf::from("settings.toml"), std::path::PathBuf::from("state"))
    }

    #[test]
    fn the_level_decides_what_isnt_set_on_its_own() {
        let mut c = config();
        assert_eq!(level(&c), Level::Subtle);
        assert!(on(&c, Feature::Beacon));
        assert!(!on(&c, Feature::CaretGlide), "glide waits for full");
        c.motion.level = Some("full".into());
        assert!(on(&c, Feature::CaretGlide));
        c.motion.level = Some("off".into());
        assert!(!on(&c, Feature::Beacon));
    }

    #[test]
    fn a_features_own_setting_wins_over_the_level() {
        let mut c = config();
        c.motion.caret_glide = Some(true);
        assert!(on(&c, Feature::CaretGlide));
        c.motion.beacon = Some(false);
        assert!(!on(&c, Feature::Beacon));
        c.motion.level = Some("off".into());
        assert!(on(&c, Feature::CaretGlide), "on by itself even with the level off");
    }

    #[test]
    fn animations_off_stops_everything() {
        let mut c = config();
        c.motion.caret_glide = Some(true);
        c.animations = Some(false);
        assert!(!on(&c, Feature::CaretGlide));
        assert_eq!(level(&c), Level::Off);
    }

    #[test]
    fn speed_scales_every_duration() {
        let mut c = config();
        assert_eq!(duration(&c, 100).as_millis(), 100);
        c.motion.speed = Some(2.0);
        assert_eq!(duration(&c, 100).as_millis(), 200);
    }

    #[test]
    fn an_anim_runs_from_zero_to_one() {
        let now = Instant::now();
        let a = Anim::new(now, Duration::from_millis(100));
        assert_eq!(a.linear(now), 0.0);
        assert!(a.running(now + Duration::from_millis(50)));
        assert!((a.linear(now + Duration::from_millis(50)) - 0.5).abs() < 1e-3);
        assert!(a.eased(now + Duration::from_millis(50)) > 0.5, "eased is ahead of linear");
        assert_eq!(a.linear(now + Duration::from_millis(200)), 1.0);
        assert!(!a.running(now + Duration::from_millis(100)));
    }
}
