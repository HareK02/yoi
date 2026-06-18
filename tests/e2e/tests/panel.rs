use std::time::{Duration, Instant};

const FIRST_VISIBLE_RENDER_BUDGET: Duration = Duration::from_millis(1500);
const DASHBOARD_CONTENT_READY_BUDGET: Duration = Duration::from_secs(5);

use yoi_e2e::{
    ExpectedPanelTicketRow, FixtureCleanupReport, FixtureWorkspace, KeyPress, PanelHarness,
    PanelRect, PanelRowKey, RenderedPanelRow, RowsRendered, yoi_binary,
};

#[test]
fn panel_fixture_ticket_row_matcher_rejects_absent_fixture_data() {
    let expected = ExpectedPanelTicketRow::new("0000000000000", "Ready E2E Ticket", "ready");
    let wrong_title = RenderedPanelRow {
        key: PanelRowKey {
            kind: "ticket".to_string(),
            id: "0000000000000".to_string(),
        },
        title: "Different Ticket".to_string(),
        status: Some("ready".to_string()),
        action: None,
        rect: PanelRect {
            x: 0,
            y: 0,
            width: 10,
            height: 1,
        },
    };
    let wrong_kind = RenderedPanelRow {
        key: PanelRowKey {
            kind: "pod".to_string(),
            id: "0000000000000".to_string(),
        },
        title: "Ready E2E Ticket".to_string(),
        status: Some("ready".to_string()),
        action: None,
        rect: PanelRect {
            x: 0,
            y: 0,
            width: 10,
            height: 1,
        },
    };

    assert!(!expected.matches(&wrong_title));
    assert!(!expected.matches(&wrong_kind));
    let rows = RowsRendered {
        selected: None,
        rows: vec![wrong_title, wrong_kind],
    };
    assert!(!rows.has_fixture_ticket_row(&expected));
}

#[test]
fn panel_first_visible_render_arrives_before_background_reload() -> yoi_e2e::Result<()> {
    let binary = yoi_binary()?;
    let fixture = FixtureWorkspace::new(&binary)?;
    assert_fixture_paths_are_isolated(&fixture);
    let ready_ticket = fixture.ready_fixture_ticket_row();

    let started = Instant::now();
    let mut panel =
        PanelHarness::spawn(fixture.panel_config_holding_background_task(binary, "reload"))?;
    let remaining = FIRST_VISIBLE_RENDER_BUDGET
        .checked_sub(started.elapsed())
        .unwrap_or_else(|| Duration::from_millis(0));
    panel.wait_for_first_visible_frame(remaining)?;
    let first_visible_elapsed = started.elapsed();
    eprintln!(
        "panel first visible frame: {first_visible_elapsed:?} (budget {FIRST_VISIBLE_RENDER_BUDGET:?}); artifacts at {}",
        panel.artifacts().dir.display()
    );
    assert!(
        first_visible_elapsed <= FIRST_VISIBLE_RENDER_BUDGET,
        "first visible frame took {first_visible_elapsed:?}, budget {FIRST_VISIBLE_RENDER_BUDGET:?}; artifacts at {}",
        panel.artifacts().dir.display()
    );

    let events = panel.events()?;
    let ready_index = events
        .iter()
        .position(|event| event.event == "panel_ready")
        .expect("panel_ready event should be present");
    assert!(
        events[..ready_index]
            .iter()
            .all(|event| event.event != "background_task_started"),
        "initial frame must be emitted before reload/background work starts; artifacts at {}",
        panel.artifacts().dir.display()
    );

    panel.expect_background_task_pending("reload")?;
    let events = panel.events()?;
    let reload_started_index = events
        .iter()
        .position(|event| {
            event.event == "background_task_started"
                && event.data.get("task").and_then(serde_json::Value::as_str) == Some("reload")
        })
        .expect("held reload should start after first visible frame");
    assert!(
        ready_index < reload_started_index,
        "first visible frame and reload ordering should remain separate; artifacts at {}",
        panel.artifacts().dir.display()
    );
    panel.assert_fixture_ticket_row_not_rendered(&ready_ticket, Duration::from_millis(150))?;

    panel.press(KeyPress::CtrlC)?;
    let status = panel.expect_exit_within(PanelHarness::default_exit_wait())?;
    assert!(status.success(), "panel should exit cleanly with Ctrl+C");
    drop(panel);
    assert_fixture_cleanup(fixture.cleanup()?);
    Ok(())
}

#[test]
fn panel_dashboard_content_ready_has_startup_budget() -> yoi_e2e::Result<()> {
    let binary = yoi_binary()?;
    let fixture = FixtureWorkspace::new(&binary)?;
    assert_fixture_paths_are_isolated(&fixture);
    let expected_content = fixture.expected_dashboard_content();

    let started = Instant::now();
    let mut panel = PanelHarness::spawn(fixture.panel_config(binary))?;
    let first_visible_remaining = FIRST_VISIBLE_RENDER_BUDGET
        .checked_sub(started.elapsed())
        .unwrap_or_else(|| Duration::from_millis(0));
    panel.wait_for_first_visible_frame(first_visible_remaining)?;
    let first_visible_elapsed = started.elapsed();
    eprintln!(
        "panel first visible frame: {first_visible_elapsed:?} (budget {FIRST_VISIBLE_RENDER_BUDGET:?}); artifacts at {}",
        panel.artifacts().dir.display()
    );
    assert!(
        first_visible_elapsed <= FIRST_VISIBLE_RENDER_BUDGET,
        "first visible frame took {first_visible_elapsed:?}, budget {FIRST_VISIBLE_RENDER_BUDGET:?}; artifacts at {}",
        panel.artifacts().dir.display()
    );

    let content_ready_remaining = DASHBOARD_CONTENT_READY_BUDGET
        .checked_sub(started.elapsed())
        .unwrap_or_else(|| Duration::from_millis(0));
    let content_ready =
        panel.wait_for_dashboard_content_ready(&expected_content, content_ready_remaining)?;
    assert!(
        content_ready.ticket_configured,
        "dashboard content ready must include usable Ticket configuration; artifacts at {}",
        panel.artifacts().dir.display()
    );
    assert!(
        content_ready.categories.ready_ticket_rows > 0
            && content_ready.categories.planning_ticket_rows > 0
            && content_ready.categories.pod_rows > 0,
        "dashboard content ready must include ready Ticket, planning Ticket, and Pod categories; got {:?}; artifacts at {}",
        content_ready.categories,
        panel.artifacts().dir.display()
    );
    let content_ready_elapsed = started.elapsed();
    eprintln!(
        "panel dashboard content ready: {content_ready_elapsed:?} (budget {DASHBOARD_CONTENT_READY_BUDGET:?}; first frame {first_visible_elapsed:?}); artifacts at {}",
        panel.artifacts().dir.display()
    );
    assert!(
        content_ready_elapsed <= DASHBOARD_CONTENT_READY_BUDGET,
        "dashboard content ready took {content_ready_elapsed:?}, budget {DASHBOARD_CONTENT_READY_BUDGET:?}; artifacts at {}",
        panel.artifacts().dir.display()
    );

    let source_breakdown = panel.expect_dashboard_source_breakdown()?;
    assert!(
        source_breakdown.has_source("pod_list.initial")
            && source_breakdown.has_source("ticket_config")
            && source_breakdown.has_source("workspace_panel.build"),
        "dashboard source breakdown should include pod, ticket, and panel-build sources; got {:?}; artifacts at {}",
        source_breakdown,
        panel.artifacts().dir.display()
    );
    eprintln!(
        "panel dashboard source breakdown: {:?}; artifacts at {}",
        source_breakdown,
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
    let events = panel.events()?;
    assert!(
        events.iter().any(|event| event.event == "quit_requested"),
        "quit_requested observability event missing; artifacts at {}",
        panel.artifacts().dir.display()
    );
    assert!(
        events.iter().any(|event| {
            event.event == "background_task_aborted"
                && event.data.get("task").and_then(serde_json::Value::as_str) == Some("reload")
        }),
        "pending reload task should be aborted before quit completes; artifacts at {}",
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
