use std::fs;
use std::time::{Duration, Instant};

use serde_json::Value;
use yoi_e2e::{FixtureCleanupReport, FixtureWorkspace, KeyPress, PanelHarness, yoi_binary};

const FIRST_VISIBLE_RENDER_BUDGET: Duration = Duration::from_millis(1500);
const DASHBOARD_CONTENT_READY_BUDGET: Duration = Duration::from_secs(5);

#[test]
fn backend_panel_loads_backend_owned_tickets_and_supports_reload_navigation_and_ctrl_c()
-> yoi_e2e::Result<()> {
    let binary = yoi_binary()?;
    let fixture = FixtureWorkspace::new(&binary)?;
    assert_backend_fixture_is_isolated(&fixture);
    let ready_ticket_id = fixture.ready_ticket_id.clone();
    let planning_ticket_id = fixture.planning_ticket_id.clone();
    let (_, workspace_id) = fixture
        .backend_target()
        .expect("panel fixture should own a Backend target");
    let workspace_id = workspace_id.to_string();

    let started = Instant::now();
    let mut panel = PanelHarness::spawn(fixture.panel_config(binary))?;
    panel.wait_for_first_visible_frame(FIRST_VISIBLE_RENDER_BUDGET)?;
    let first_visible_elapsed = started.elapsed();
    assert!(
        first_visible_elapsed <= FIRST_VISIBLE_RENDER_BUDGET,
        "first visible Backend panel frame took {first_visible_elapsed:?}, budget {FIRST_VISIBLE_RENDER_BUDGET:?}; artifacts at {}",
        panel.artifacts().dir.display()
    );

    let ready = panel.wait_for(
        "Backend dashboard content ready with fixture Tickets",
        DASHBOARD_CONTENT_READY_BUDGET,
        |event| {
            event.event == "backend_dashboard_content_ready"
                && event.data.get("workspace_id").and_then(Value::as_str)
                    == Some(workspace_id.as_str())
                && has_backend_ticket(
                    &event.data,
                    &ready_ticket_id,
                    "Planning E2E Ticket A",
                    "planning",
                )
                && has_backend_ticket(
                    &event.data,
                    &planning_ticket_id,
                    "Planning E2E Ticket B",
                    "planning",
                )
        },
    )?;
    assert_eq!(
        ready.data.get("focus").and_then(Value::as_str),
        Some("tickets")
    );
    assert!(
        started.elapsed() <= DASHBOARD_CONTENT_READY_BUDGET,
        "Backend dashboard content missed startup budget; artifacts at {}",
        panel.artifacts().dir.display()
    );

    panel.press(KeyPress::Text("j".to_string()))?;
    panel.wait_for(
        "Backend dashboard Ticket selection change",
        Duration::from_secs(2),
        |event| {
            event.event == "backend_dashboard_selection_changed"
                && event.data.get("selected_ticket_id").and_then(Value::as_str)
                    == Some(planning_ticket_id.as_str())
        },
    )?;

    panel.press(KeyPress::Text("r".to_string()))?;
    panel.wait_for(
        "Backend dashboard reload",
        Duration::from_secs(2),
        |event| {
            event.event == "backend_dashboard_reloaded"
                && has_backend_ticket(
                    &event.data,
                    &ready_ticket_id,
                    "Planning E2E Ticket A",
                    "planning",
                )
                && has_backend_ticket(
                    &event.data,
                    &planning_ticket_id,
                    "Planning E2E Ticket B",
                    "planning",
                )
        },
    )?;

    panel.press(KeyPress::CtrlC)?;
    let status = panel.expect_exit_within(PanelHarness::default_exit_wait())?;
    assert!(
        status.success(),
        "Backend panel should exit cleanly with Ctrl+C"
    );
    assert!(
        panel
            .events()?
            .iter()
            .any(|event| event.event == "quit_requested")
    );
    drop(panel);

    let report = fixture.cleanup()?;
    assert_fixture_cleanup(&report);
    assert!(
        !report
            .snapshot_dir
            .join("config/yoi/backend-tokens.json")
            .exists(),
        "Backend credential must be removed before fixture snapshot"
    );
    Ok(())
}

#[test]
fn backend_panel_shell_enter_path_uses_the_same_backend_authority() -> yoi_e2e::Result<()> {
    let binary = yoi_binary()?;
    let fixture = FixtureWorkspace::new(&binary)?;
    assert_backend_fixture_is_isolated(&fixture);
    let ready_ticket_id = fixture.ready_ticket_id.clone();

    let (mut panel, started) = PanelHarness::spawn_via_shell_enter(fixture.panel_config(binary))?;
    panel.wait_for_first_visible_frame(FIRST_VISIBLE_RENDER_BUDGET)?;
    panel.wait_for(
        "shell-enter Backend dashboard content",
        DASHBOARD_CONTENT_READY_BUDGET,
        |event| {
            event.event == "backend_dashboard_content_ready"
                && has_backend_ticket(
                    &event.data,
                    &ready_ticket_id,
                    "Planning E2E Ticket A",
                    "planning",
                )
        },
    )?;
    assert!(
        started.elapsed() <= DASHBOARD_CONTENT_READY_BUDGET,
        "shell-enter Backend dashboard content missed startup budget; artifacts at {}",
        panel.artifacts().dir.display()
    );

    panel.press(KeyPress::Esc)?;
    let status = panel.expect_exit_within(PanelHarness::default_exit_wait())?;
    assert!(
        status.success(),
        "Backend panel should exit cleanly with Escape"
    );
    drop(panel);
    let report = fixture.cleanup()?;
    assert_fixture_cleanup(&report);
    Ok(())
}

fn has_backend_ticket(data: &Value, id: &str, title: &str, workflow_state: &str) -> bool {
    data.get("tickets")
        .and_then(Value::as_array)
        .is_some_and(|tickets| {
            tickets.iter().any(|ticket| {
                ticket.get("id").and_then(Value::as_str) == Some(id)
                    && ticket.get("title").and_then(Value::as_str) == Some(title)
                    && ticket.get("workflow_state").and_then(Value::as_str) == Some(workflow_state)
            })
        })
}

fn assert_backend_fixture_is_isolated(fixture: &FixtureWorkspace) {
    assert!(fixture.root.exists());
    assert!(fixture.workspace.starts_with(&fixture.root));
    assert!(fixture.home.starts_with(&fixture.root));
    assert!(fixture.xdg_data_home.starts_with(&fixture.root));
    assert!(fixture.xdg_state_home.starts_with(&fixture.root));
    assert!(fixture.xdg_config_home.starts_with(&fixture.root));
    assert!(fixture.xdg_runtime_dir.starts_with(&fixture.root));
    assert!(!fixture.workspace.join(".yoi").exists());
    assert!(fixture.backend_target().is_some());
}

fn assert_fixture_cleanup(report: &FixtureCleanupReport) {
    assert!(
        report.cleanup_success,
        "fixture cleanup failed; report at {}: {:?}",
        report.report_path.display(),
        report.cleanup_error
    );
    assert!(!report.fixture_root.exists());
    assert!(report.report_path.exists());
    assert!(report.snapshot_dir.exists());
    let snapshot = fs::read_to_string(report.snapshot_dir.join("../fixture.json")).unwrap();
    assert!(!snapshot.contains("yoi_api_"));
}
