use std::time::Duration;

use yoi_e2e::{FixtureWorkspace, KeyPress, PanelHarness, yoi_binary};

#[test]
fn single_pod_rewind_picker_applies_without_escape_and_suppresses_duplicate_enter()
-> yoi_e2e::Result<()> {
    let binary = yoi_binary()?;
    let fixture = FixtureWorkspace::new(&binary)?;
    let mut tui = PanelHarness::spawn(fixture.rewind_fixture_config(binary))?;

    tui.expect_mouse_capture_enabled()?;
    tui.assert_no_full_drag_mouse_capture()?;
    tui.expect_event("rewind_fixture_ready", Duration::from_secs(5))?;

    let before_rewind_output = tui.output_len();
    tui.press(KeyPress::CtrlR)?;
    tui.expect_event("rewind_picker_opened", Duration::from_secs(5))?;

    tui.press(KeyPress::Enter)?;
    tui.expect_event("rewind_submit_sent", Duration::from_secs(5))?;
    let submit_count = tui.count_events("rewind_submit_sent")?;

    tui.press(KeyPress::Enter)?;
    tui.expect_event("rewind_duplicate_enter_suppressed", Duration::from_secs(5))?;
    tui.wait_for_no_additional_events(
        "rewind_submit_sent",
        submit_count,
        Duration::from_millis(250),
    )?;

    let applied = tui.expect_event("rewind_applied", Duration::from_secs(5))?;
    assert_eq!(
        applied
            .data
            .get("composer_text")
            .and_then(serde_json::Value::as_str),
        Some("rewind-live-refresh"),
        "rewind should update the visible composer state without Esc/restart; artifacts at {}",
        tui.artifacts().dir.display()
    );
    tui.wait_for_output_contains_from(
        before_rewind_output,
        "rewind-live-refresh",
        Duration::from_secs(5),
    )?;
    assert_eq!(
        tui.count_events("rewind_submit_sent")?,
        submit_count,
        "pending Enter must not duplicate destructive rewind submissions; artifacts at {}",
        tui.artifacts().dir.display()
    );

    tui.press(KeyPress::CtrlD)?;
    let status = tui.expect_exit_within(PanelHarness::default_exit_wait())?;
    assert!(
        status.success(),
        "single-worker rewind fixture should exit cleanly"
    );
    drop(tui);
    let cleanup = fixture.cleanup()?;
    assert!(
        cleanup.cleanup_success,
        "fixture cleanup failed: {cleanup:?}"
    );
    Ok(())
}
