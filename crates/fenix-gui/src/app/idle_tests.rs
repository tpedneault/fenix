//! An idle editor draws nothing and sleeps (#74): the loop's own plan,
//! and what comes back on its own every couple of seconds.

use super::git_editor::ChromeGit;
use super::*;

/// With nothing typed for a while, nothing animating and nothing due, the
/// loop plans no frame and sleeps until the next disk check at the soonest
/// -- seconds, not the 16 ms of an animation or the 500 ms of a blink.
#[test]
fn an_idle_editor_plans_no_frame_and_a_long_sleep() {
    let mut app = App::with_file(None);
    let now = Instant::now();
    // The last key was long ago: the caret has blinked its last.
    app.caret_woken = now - CARET_BLINKS_FOR - Duration::from_secs(1);
    app.blink_visible = true;
    app.blink_transition_start = now - Duration::from_secs(5);
    app.next_disk_check = now + DISK_CHECK_INTERVAL;
    // What the first frames leave due, done.
    app.plan_wake(now);
    let mut frames = 0;
    let mut slept = Duration::ZERO;
    let mut at = now;
    // A minute of waking only when the plan says to.
    while at < now + Duration::from_secs(60) {
        let plan = app.plan_wake(at);
        frames += usize::from(plan.redraw);
        assert!(plan.wait_until > at, "it would spin: {plan:?}");
        slept = slept.max(plan.wait_until - at);
        at = plan.wait_until;
    }
    assert_eq!(frames, 0, "an idle editor drew frames");
    assert!(
        slept >= Duration::from_secs(1),
        "it never slept for long: at most {slept:?}"
    );
}

/// The same, but just after a key: the caret blinks, so frames are
/// planned -- what the test above shows stops is real.
#[test]
fn a_caret_just_woken_does_plan_frames() {
    let mut app = App::with_file(None);
    let now = Instant::now();
    app.wake_caret();
    let mut frames = 0;
    let mut at = now;
    while at < now + Duration::from_secs(5) {
        let plan = app.plan_wake(at);
        frames += usize::from(plan.redraw);
        at = plan.wait_until.max(at + Duration::from_millis(1));
    }
    assert!(frames > 0, "the caret didn't blink");
}

/// The modeline's Git state, read again every couple of seconds, asks for
/// a frame only when it's different.
#[test]
fn the_same_git_state_for_the_modeline_asks_for_no_frame() {
    let mut app = App::with_file(None);
    let state = ChromeGit {
        root: PathBuf::from("/work/repo"),
        buffer: app.focused_buffer_id(),
        branch: "main".into(),
        tracking: Some((0, 0)),
        dirty: 2,
        op: None,
        changes: 7,
    };
    assert!(
        app.apply_chrome_git(Some(state.clone())),
        "the first read shows"
    );
    assert!(
        !app.apply_chrome_git(Some(state.clone())),
        "the same again doesn't"
    );
    assert!(
        app.apply_chrome_git(Some(ChromeGit { dirty: 3, ..state })),
        "a change shows"
    );
    assert!(app.apply_chrome_git(None), "out of the repository shows");
    assert!(!app.apply_chrome_git(None));
}
