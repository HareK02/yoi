use std::time::Duration;

use yoi_e2e::{FixtureWorkspace, KeyPress, PanelHarness, yoi_binary};

#[test]
fn panel_mouse_click_selects_row_without_dispatching_action() -> yoi_e2e::Result<()> {
    let binary = yoi_binary();
    let fixture = FixtureWorkspace::new(&binary)?;
    let mut panel = PanelHarness::spawn(fixture.panel_config(binary))?;

    let rows = panel.wait_for_rows(2)?;
    let selected = rows.selected.clone();
    let target = rows
        .rows
        .iter()
        .find(|row| Some(&row.key) != selected.as_ref())
        .cloned()
        .expect("fixture should render a second selectable row");

    let before_events = panel.events()?.len();
    panel.click(&target)?;
    panel.expect_selection(&target.key)?;

    let events = panel.events()?;
    assert!(
        events[before_events..]
            .iter()
            .all(|event| event.event != "action_requested"),
        "mouse selection must not dispatch panel actions; artifacts at {}",
        panel.artifacts().dir.display()
    );

    panel.press(KeyPress::CtrlC)?;
    let status = panel.expect_exit_within(PanelHarness::default_exit_wait())?;
    assert!(status.success(), "panel should exit cleanly with Ctrl+C");
    Ok(())
}

#[test]
fn panel_ctrl_c_exits_promptly_after_background_barrier() -> yoi_e2e::Result<()> {
    let binary = yoi_binary();
    let fixture = FixtureWorkspace::new(&binary)?;
    let mut panel = PanelHarness::spawn(fixture.panel_config(binary))?;

    panel.wait_for("panel_ready", Duration::from_secs(5), |event| {
        event.event == "panel_ready"
    })?;
    assert!(
        panel
            .events()?
            .iter()
            .any(|event| event.event == "background_task_started"),
        "background task barrier was not observed; artifacts at {}",
        panel.artifacts().dir.display()
    );

    let started = std::time::Instant::now();
    panel.press(KeyPress::CtrlC)?;
    let status = panel.expect_exit_within(PanelHarness::default_exit_wait())?;
    let elapsed = started.elapsed();

    assert!(status.success(), "panel should exit cleanly with Ctrl+C");
    assert!(
        elapsed <= PanelHarness::default_exit_wait(),
        "quit latency {elapsed:?} exceeded threshold; artifacts at {}",
        panel.artifacts().dir.display()
    );
    assert!(
        panel
            .events()?
            .iter()
            .any(|event| event.event == "quit_requested"),
        "quit_requested observability event missing; artifacts at {}",
        panel.artifacts().dir.display()
    );
    Ok(())
}
