use std::io;
use std::time::Duration;

use client::{BackendWorkspaceProductClient, ObjectiveSummary};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};
use ticket::{TicketListQuery, TicketSummary};

use crate::console::{enter_dashboard_fullscreen, leave_dashboard_fullscreen};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Tickets,
    Objectives,
}

struct BackendDashboard {
    workspace_id: String,
    tickets: Vec<TicketSummary>,
    objectives: Vec<ObjectiveSummary>,
    focus: Focus,
    selected_ticket: usize,
    selected_objective: usize,
    status: String,
}

pub async fn launch(
    base_url: String,
    workspace_id: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = BackendWorkspaceProductClient::new(base_url, workspace_id.clone())?;
    let (tickets, objectives) = load(&client).await?;
    let mut app = BackendDashboard {
        workspace_id,
        tickets,
        objectives,
        focus: Focus::Tickets,
        selected_ticket: 0,
        selected_objective: 0,
        status: "Backend Ticket and Objective authority selected".to_string(),
    };

    let mut terminal = enter_dashboard_fullscreen()?;
    let result = run_loop(&mut terminal, &mut app, client).await;
    let restore_result = leave_dashboard_fullscreen(&mut terminal);
    result?;
    restore_result?;
    Ok(())
}

async fn load(
    client: &BackendWorkspaceProductClient,
) -> Result<(Vec<TicketSummary>, Vec<ObjectiveSummary>), Box<dyn std::error::Error>> {
    let client = client.clone();
    tokio::task::spawn_blocking(move || {
        let tickets = client
            .list_tickets(&TicketListQuery::active())
            .map_err(|error| error.to_string())?;
        let objectives = client
            .list_objectives(BackendWorkspaceProductClient::default_product_list_limit())
            .map_err(|error| error.to_string())?
            .items;
        Ok::<_, String>((tickets, objectives))
    })
    .await
    .map_err(|error| format!("Backend dashboard loading task failed: {error}"))?
    .map_err(Into::into)
}

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut BackendDashboard,
    client: BackendWorkspaceProductClient,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut first_frame = true;
    let mut initial_snapshot_pending = false;
    loop {
        terminal.draw(|frame| draw(frame, app))?;
        if first_frame {
            first_frame = false;
            #[cfg(feature = "e2e-test")]
            {
                crate::e2e_observer::emit(
                    "backend_dashboard",
                    "panel_ready",
                    serde_json::json!({ "workspace_id": app.workspace_id }),
                );
                initial_snapshot_pending = true;
            }
        } else if initial_snapshot_pending {
            #[cfg(feature = "e2e-test")]
            emit_e2e_snapshot(app, "backend_dashboard_content_ready");
            initial_snapshot_pending = false;
        }
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                #[cfg(feature = "e2e-test")]
                crate::e2e_observer::emit(
                    "backend_dashboard",
                    "quit_requested",
                    serde_json::json!({}),
                );
                return Ok(());
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                #[cfg(feature = "e2e-test")]
                crate::e2e_observer::emit(
                    "backend_dashboard",
                    "quit_requested",
                    serde_json::json!({}),
                );
                return Ok(());
            }
            KeyCode::Tab | KeyCode::BackTab => {
                app.focus = match app.focus {
                    Focus::Tickets => Focus::Objectives,
                    Focus::Objectives => Focus::Tickets,
                };
                #[cfg(feature = "e2e-test")]
                emit_e2e_snapshot(app, "backend_dashboard_selection_changed");
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.select_next();
                #[cfg(feature = "e2e-test")]
                emit_e2e_snapshot(app, "backend_dashboard_selection_changed");
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.select_previous();
                #[cfg(feature = "e2e-test")]
                emit_e2e_snapshot(app, "backend_dashboard_selection_changed");
            }
            KeyCode::Char('r') => match load(&client).await {
                Ok((tickets, objectives)) => {
                    app.tickets = tickets;
                    app.objectives = objectives;
                    app.clamp_selection();
                    app.status = "Reloaded from Backend authority".to_string();
                    #[cfg(feature = "e2e-test")]
                    emit_e2e_snapshot(app, "backend_dashboard_reloaded");
                }
                Err(error) => app.status = format!("Backend reload failed: {error}"),
            },
            KeyCode::Char('i') if app.focus == Focus::Tickets => {
                let Some(ticket_id) = app.selected_ticket_id() else {
                    app.status = "Select a Ticket before launching Intake".to_string();
                    continue;
                };
                let client = client.clone();
                match tokio::task::spawn_blocking(move || client.launch_ticket_intake(&ticket_id))
                    .await
                {
                    Ok(Ok(status)) => app.status = status,
                    Ok(Err(error)) => app.status = format!("Backend Intake launch failed: {error}"),
                    Err(error) => {
                        app.status = format!("Backend Intake launch task failed: {error}")
                    }
                }
            }
            KeyCode::Char('o') => {
                let client = client.clone();
                match tokio::task::spawn_blocking(move || client.start_workspace_orchestrator())
                    .await
                {
                    Ok(Ok(status)) => app.status = status,
                    Ok(Err(error)) => {
                        app.status = format!("Backend Orchestrator launch failed: {error}")
                    }
                    Err(error) => {
                        app.status = format!("Backend Orchestrator launch task failed: {error}")
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(feature = "e2e-test")]
fn emit_e2e_snapshot(app: &BackendDashboard, event: &'static str) {
    let focus = match app.focus {
        Focus::Tickets => "tickets",
        Focus::Objectives => "objectives",
    };
    let tickets = app
        .tickets
        .iter()
        .map(|ticket| {
            serde_json::json!({
                "id": ticket.id,
                "resource_key": ticket.resource_key,
                "title": ticket.title,
                "workflow_state": ticket.workflow_state.as_str(),
            })
        })
        .collect::<Vec<_>>();
    let objectives = app
        .objectives
        .iter()
        .map(|objective| {
            serde_json::json!({
                "id": objective.id,
                "resource_key": objective.resource_key,
                "title": objective.title,
                "state": objective.state,
            })
        })
        .collect::<Vec<_>>();
    crate::e2e_observer::emit(
        "backend_dashboard",
        event,
        serde_json::json!({
            "workspace_id": app.workspace_id,
            "focus": focus,
            "selected_ticket_id": app.selected_ticket_id(),
            "selected_objective_id": app.objectives.get(app.selected_objective).map(|objective| &objective.id),
            "tickets": tickets,
            "objectives": objectives,
            "status": app.status,
        }),
    );
}

impl BackendDashboard {
    fn select_next(&mut self) {
        match self.focus {
            Focus::Tickets if !self.tickets.is_empty() => {
                self.selected_ticket = (self.selected_ticket + 1).min(self.tickets.len() - 1);
            }
            Focus::Objectives if !self.objectives.is_empty() => {
                self.selected_objective =
                    (self.selected_objective + 1).min(self.objectives.len() - 1);
            }
            _ => {}
        }
    }

    fn select_previous(&mut self) {
        match self.focus {
            Focus::Tickets => self.selected_ticket = self.selected_ticket.saturating_sub(1),
            Focus::Objectives => {
                self.selected_objective = self.selected_objective.saturating_sub(1)
            }
        }
    }

    fn selected_ticket_id(&self) -> Option<String> {
        self.tickets
            .get(self.selected_ticket)
            .map(|ticket| ticket.id.clone())
    }

    fn clamp_selection(&mut self) {
        self.selected_ticket = self
            .selected_ticket
            .min(self.tickets.len().saturating_sub(1));
        self.selected_objective = self
            .selected_objective
            .min(self.objectives.len().saturating_sub(1));
    }
}

fn draw(frame: &mut Frame<'_>, app: &BackendDashboard) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(4),
        ])
        .split(frame.area());
    frame.render_widget(
        Paragraph::new(format!(
            "Workspace {} · Backend product state",
            app.workspace_id
        ))
        .block(Block::default().borders(Borders::ALL).title("Panel")),
        areas[0],
    );

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(areas[1]);
    draw_tickets(frame, columns[0], app);
    draw_objectives(frame, columns[1], app);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from("Tab switch · j/k move · r reload · i Intake · o Orchestrator · q quit"),
            Line::from(app.status.as_str()),
        ])
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title("Status")),
        areas[2],
    );
}

fn draw_tickets(frame: &mut Frame<'_>, area: Rect, app: &BackendDashboard) {
    let items = app.tickets.iter().enumerate().map(|(index, ticket)| {
        let marker = if index == app.selected_ticket {
            ">"
        } else {
            " "
        };
        ListItem::new(format!(
            "{marker} {} [{}] {}",
            ticket.resource_key.as_deref().unwrap_or(&ticket.id),
            ticket.workflow_state.as_str(),
            ticket.title
        ))
    });
    let style = focus_style(app.focus == Focus::Tickets);
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(style)
                .title(format!("Tickets ({})", app.tickets.len())),
        ),
        area,
    );
}

fn draw_objectives(frame: &mut Frame<'_>, area: Rect, app: &BackendDashboard) {
    let items = app.objectives.iter().enumerate().map(|(index, objective)| {
        let marker = if index == app.selected_objective {
            ">"
        } else {
            " "
        };
        ListItem::new(format!(
            "{marker} {} [{}] {}",
            objective.resource_key, objective.state, objective.title
        ))
    });
    let style = focus_style(app.focus == Focus::Objectives);
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(style)
                .title(format!("Objectives ({})", app.objectives.len())),
        ),
        area,
    );
}

fn focus_style(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_dashboard_navigation_is_bounded() {
        let mut app = BackendDashboard {
            workspace_id: "workspace-a".to_string(),
            tickets: Vec::new(),
            objectives: Vec::new(),
            focus: Focus::Tickets,
            selected_ticket: 0,
            selected_objective: 0,
            status: String::new(),
        };
        app.select_next();
        app.select_previous();
        assert_eq!(app.selected_ticket, 0);
        assert_eq!(app.selected_objective, 0);
    }
}
