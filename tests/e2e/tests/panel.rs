use std::time::Duration;

use yoi_e2e::{
    FixtureCleanupReport, FixtureWorkspace, KeyPress, PanelHarness, RenderedPanelRow, yoi_binary,
};

#[test]
fn panel_mouse_click_selects_row_without_dispatching_action() -> yoi_e2e::Result<()> {
    let binary = yoi_binary()?;
    let fixture = FixtureWorkspace::new(&binary)?;
    assert_fixture_paths_are_isolated(&fixture);
    let mut panel = PanelHarness::spawn(fixture.panel_config(binary))?;

    panel.expect_mouse_capture_enabled()?;
    panel.assert_no_full_drag_mouse_capture()?;
    let rows = panel.wait_for_rows(2)?;
    assert_no_runtime_or_host_pod_leak(
        &fixture,
        &rows.rows,
        panel.artifacts().dir.display().to_string().as_str(),
    );
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
    panel.assert_no_full_drag_mouse_capture()?;

    panel.press(KeyPress::CtrlC)?;
    let status = panel.expect_exit_within(PanelHarness::default_exit_wait())?;
    assert!(status.success(), "panel should exit cleanly with Ctrl+C");
    drop(panel);
    assert_fixture_cleanup(fixture.cleanup()?);
    Ok(())
}

#[test]
fn panel_mouse_wheel_moves_selection_without_full_drag_capture() -> yoi_e2e::Result<()> {
    let binary = yoi_binary()?;
    let fixture = FixtureWorkspace::new(&binary)?;
    assert_fixture_paths_are_isolated(&fixture);
    let mut panel = PanelHarness::spawn(fixture.panel_config(binary))?;

    panel.expect_mouse_capture_enabled()?;
    panel.assert_no_full_drag_mouse_capture()?;
    let rows = panel.wait_for_rows(2)?;
    assert_no_runtime_or_host_pod_leak(
        &fixture,
        &rows.rows,
        panel.artifacts().dir.display().to_string().as_str(),
    );
    let selected = rows
        .selected
        .as_ref()
        .expect("fixture should render an initially selected row")
        .clone();
    let selected_index = rows
        .rows
        .iter()
        .position(|row| row.key == selected)
        .expect("selected row should be rendered");
    let target_index = (selected_index + 1).min(rows.rows.len() - 1);
    assert_ne!(
        selected_index, target_index,
        "fixture should render a wheel-selectable next row"
    );
    let source = rows.rows[selected_index].clone();
    let target = rows.rows[target_index].clone();

    let before_events = panel.events()?.len();
    panel.wheel_down(&source)?;
    panel.expect_selection(&target.key)?;
    panel.assert_no_full_drag_mouse_capture()?;

    let events = panel.events()?;
    assert!(
        events[before_events..]
            .iter()
            .any(|event| event.event == "mouse_wheel"),
        "wheel movement should be visible in e2e events; artifacts at {}",
        panel.artifacts().dir.display()
    );
    assert!(
        events[before_events..]
            .iter()
            .all(|event| event.event != "action_requested"),
        "wheel selection must not dispatch panel actions; artifacts at {}",
        panel.artifacts().dir.display()
    );

    panel.press(KeyPress::CtrlC)?;
    let status = panel.expect_exit_within(PanelHarness::default_exit_wait())?;
    assert!(status.success(), "panel should exit cleanly with Ctrl+C");
    drop(panel);
    assert_fixture_cleanup(fixture.cleanup()?);
    Ok(())
}

#[test]
fn panel_ctrl_c_exits_promptly_after_background_barrier() -> yoi_e2e::Result<()> {
    let binary = yoi_binary()?;
    let fixture = FixtureWorkspace::new(&binary)?;
    assert_fixture_paths_are_isolated(&fixture);
    let mut panel =
        PanelHarness::spawn(fixture.panel_config_holding_background_task(binary, "reload"))?;

    panel.wait_for("panel_ready", Duration::from_secs(5), |event| {
        event.event == "panel_ready"
    })?;
    panel.expect_background_task_pending("reload")?;

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
    drop(panel);
    assert_fixture_cleanup(fixture.cleanup()?);
    Ok(())
}

fn assert_fixture_paths_are_isolated(fixture: &FixtureWorkspace) {
    assert!(
        fixture.root.exists(),
        "fixture temp root should exist during scenario"
    );
    assert!(fixture.workspace.starts_with(&fixture.root));
    assert!(fixture.home.starts_with(&fixture.root));
    assert!(fixture.xdg_data_home.starts_with(&fixture.root));
    assert!(fixture.xdg_state_home.starts_with(&fixture.root));
    assert!(fixture.xdg_config_home.starts_with(&fixture.root));
    assert!(fixture.xdg_runtime_dir.starts_with(&fixture.root));
    assert!(
        !fixture.artifacts_dir.starts_with(&fixture.root),
        "persistent artifacts must live outside the temp root so cleanup can remove the fixture"
    );
    if let Some(host_runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        assert_ne!(
            fixture.xdg_runtime_dir,
            std::path::PathBuf::from(host_runtime),
            "tested yoi must not reuse host XDG_RUNTIME_DIR"
        );
    }
}

fn assert_no_runtime_or_host_pod_leak(
    fixture: &FixtureWorkspace,
    rows: &[RenderedPanelRow],
    artifacts: &str,
) {
    let rendered = rows
        .iter()
        .map(|row| {
            format!(
                "{} {} {} {}",
                row.key.kind,
                row.key.id,
                row.title,
                row.status.as_deref().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    for marker in [
        "workspace-orchestrator",
        "yoi-orchestrator-orchestrator",
        "host-runtime-leak",
    ] {
        assert!(
            !rendered.contains(marker),
            "host/fixture runtime Pod marker {marker:?} leaked into panel rows; artifacts at {artifacts}\n{rendered}"
        );
    }
    if let Some(host_runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let host_runtime = host_runtime.to_string_lossy();
        assert!(
            !rendered.contains(host_runtime.as_ref()),
            "host XDG_RUNTIME_DIR leaked into panel rows; artifacts at {artifacts}\n{rendered}"
        );
    }
    assert!(
        rendered.contains("E2E Ticket"),
        "panel should be observing fixture-local Ticket data; artifacts at {artifacts}\n{rendered}"
    );
    assert!(fixture.xdg_runtime_dir.starts_with(&fixture.root));
}

fn assert_fixture_cleanup(report: FixtureCleanupReport) {
    assert!(
        report.cleanup_success,
        "fixture cleanup failed; report at {}: {:?}",
        report.report_path.display(),
        report.cleanup_error
    );
    assert!(
        !report.fixture_root.exists(),
        "fixture temp root should be removed after scenario: {}",
        report.fixture_root.display()
    );
    assert!(
        report.report_path.exists(),
        "cleanup artifact should persist"
    );
    assert!(
        report.snapshot_dir.exists(),
        "fixture snapshot should persist under target/e2e-artifacts before temp cleanup"
    );
}
