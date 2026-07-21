use std::error::Error;
use std::io;

use client::{
    BackendRuntimeListTarget, BackendRuntimeTarget, BackendWorkerSummary, list_backend_workers,
};
use crossterm::event::{self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind};
use ratatui::Frame;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

use crate::console;

pub(crate) async fn run(target: BackendRuntimeListTarget) -> Result<(), Box<dyn Error>> {
    let response = list_backend_workers(&target).await.map_err(|error| {
        io::Error::other(format!(
            "failed to list Backend runtime workers from {}: {error}",
            target.base_url
        ))
    })?;
    if response.items.is_empty() {
        let diagnostics = response
            .diagnostics
            .iter()
            .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
            .collect::<Vec<_>>()
            .join("; ");
        let detail = if diagnostics.is_empty() {
            "no backend diagnostics".to_string()
        } else {
            diagnostics
        };
        return Err(Box::new(io::Error::other(format!(
            "Backend returned no runtime workers for workspace {} ({detail})",
            response.workspace_id
        ))));
    }

    let selected = pick_worker(target.clone(), response.items)?;
    let attach_target =
        BackendRuntimeTarget::new(target.base_url, selected.runtime_id, selected.worker_id);
    console::run_backend_runtime(attach_target).await
}

fn pick_worker(
    target: BackendRuntimeListTarget,
    mut workers: Vec<BackendWorkerSummary>,
) -> Result<BackendWorkerSummary, Box<dyn Error>> {
    workers.sort_by(|a, b| {
        a.runtime_id
            .cmp(&b.runtime_id)
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.worker_id.cmp(&b.worker_id))
    });
    let mut state = BackendWorkerPickerState::new(target, workers);
    let mut terminal = ratatui::Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;

    loop {
        terminal.draw(|frame| draw(frame, &mut state))?;
        match event::read()? {
            CrosstermEvent::Key(KeyEvent {
                code,
                kind: KeyEventKind::Press,
                ..
            }) => match code {
                KeyCode::Up | KeyCode::Char('k') => state.previous(),
                KeyCode::Down | KeyCode::Char('j') => state.next(),
                KeyCode::Enter => return Ok(state.selected_worker().clone()),
                KeyCode::Esc | KeyCode::Char('q') => {
                    return Err(Box::new(io::Error::other(
                        "Backend worker picker cancelled",
                    )));
                }
                _ => {}
            },
            _ => {}
        }
    }
}

struct BackendWorkerPickerState {
    target: BackendRuntimeListTarget,
    workers: Vec<BackendWorkerSummary>,
    selected: usize,
}

impl BackendWorkerPickerState {
    fn new(target: BackendRuntimeListTarget, workers: Vec<BackendWorkerSummary>) -> Self {
        Self {
            target,
            workers,
            selected: 0,
        }
    }

    fn next(&mut self) {
        if self.workers.is_empty() {
            return;
        }
        self.selected = (self.selected + 1).min(self.workers.len() - 1);
    }

    fn previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn selected_worker(&self) -> &BackendWorkerSummary {
        &self.workers[self.selected]
    }
}

fn draw(frame: &mut Frame<'_>, state: &mut BackendWorkerPickerState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(8),
            Constraint::Length(5),
        ])
        .split(frame.area());

    draw_header(frame, chunks[0], state);
    draw_list(frame, chunks[1], state);
    draw_details(frame, chunks[2], state.selected_worker());
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, state: &BackendWorkerPickerState) {
    let workspace = state
        .target
        .workspace_id
        .as_deref()
        .unwrap_or("unscoped backend");
    let runtime = state.target.runtime_id.as_deref().unwrap_or("all runtimes");
    let text = vec![
        Line::from(vec![
            Span::styled(
                "Backend runtime workers",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  {}", state.target.base_url)),
        ]),
        Line::from(format!("workspace: {workspace}  runtime: {runtime}")),
        Line::from("↑/↓ or k/j select  Enter attach  q/Esc cancel"),
    ];
    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_list(frame: &mut Frame<'_>, area: Rect, state: &mut BackendWorkerPickerState) {
    let items: Vec<_> = state
        .workers
        .iter()
        .map(|worker| ListItem::new(worker_row(worker)))
        .collect();
    let mut list_state = ListState::default().with_selected(Some(state.selected));
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Workers"))
        .highlight_symbol("▶ ")
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn worker_row(worker: &BackendWorkerSummary) -> Line<'static> {
    let label = if worker.label.is_empty() {
        worker.worker_id.as_str()
    } else {
        worker.label.as_str()
    };
    let profile = worker.profile.as_deref().unwrap_or("-");
    let wd = working_directory_text(worker);
    Line::from(vec![
        Span::styled(
            format!("{}:{}", worker.runtime_id, worker.worker_id),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(format!("  {label}")),
        Span::raw(format!("  profile:{profile}")),
        Span::raw(format!("  state:{}", worker.state)),
        Span::raw(format!("  wd:{wd}")),
    ])
}

fn draw_details(frame: &mut Frame<'_>, area: Rect, worker: &BackendWorkerSummary) {
    let profile = worker.profile.as_deref().unwrap_or("-");
    let role = worker.role.as_deref().unwrap_or("-");
    let text = vec![
        Line::from(format!(
            "runtime={} worker={} host={}",
            worker.runtime_id, worker.worker_id, worker.host_id
        )),
        Line::from(format!(
            "label={} role={} profile={} state={}",
            worker.label, role, profile, worker.state
        )),
        Line::from(format!(
            "working_directory={}",
            working_directory_text(worker)
        )),
    ];
    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Selected worker"),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn working_directory_text(worker: &BackendWorkerSummary) -> String {
    let Some(wd) = worker.working_directory.as_ref() else {
        return "-".to_string();
    };
    let cleanliness = wd.cleanliness.as_deref().unwrap_or("unknown");
    format!(
        "{}:{} {} {}",
        wd.repository_id, wd.working_directory_id, wd.status, cleanliness
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use client::{
        BackendWorkerCapabilitySummary, BackendWorkerImplementationSummary,
        BackendWorkerWorkspaceSummary,
    };

    fn worker(runtime_id: &str, worker_id: &str, profile: Option<&str>) -> BackendWorkerSummary {
        BackendWorkerSummary {
            runtime_id: runtime_id.to_string(),
            worker_id: worker_id.to_string(),
            host_id: "host".to_string(),
            label: "label".to_string(),
            role: None,
            profile: profile.map(str::to_string),
            workspace: BackendWorkerWorkspaceSummary {
                visibility: "workspace".to_string(),
                identity: "ws".to_string(),
            },
            state: "running".to_string(),
            last_seen_at: None,
            pinned: false,
            retention_state: String::new(),
            implementation: BackendWorkerImplementationSummary {
                kind: "embedded".to_string(),
                display_hint: "embedded".to_string(),
            },
            capabilities: BackendWorkerCapabilitySummary {
                can_stop: true,
                can_spawn_followup: false,
            },
            working_directory: None,
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn worker_row_contains_backend_authority_fields() {
        let row = worker_row(&worker("runtime-a", "worker-b", Some("default")));
        let text = row
            .spans
            .into_iter()
            .map(|span| span.content)
            .collect::<String>();
        assert!(text.contains("runtime-a:worker-b"));
        assert!(text.contains("profile:default"));
        assert!(text.contains("state:running"));
    }
}
