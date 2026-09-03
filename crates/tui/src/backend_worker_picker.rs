use std::error::Error;
use std::io;
use std::time::Duration;

use client::{
    BackendRuntimeListTarget, BackendWorkerSummary, list_backend_stopped_workers,
    list_backend_workers, restore_backend_worker,
};
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::backend_workspace_picker::select_backend_workspace;
use crate::console;
use crate::inline_terminal::with_inline_terminal;

const MAX_ROWS: usize = 10;
const VIEWPORT_LINES: u16 = MAX_ROWS as u16 + 4;

pub(crate) async fn run(
    mut target: BackendRuntimeListTarget,
    include_stopped: bool,
) -> Result<(), Box<dyn Error>> {
    loop {
        if target.workspace_id().is_none() {
            let workspace_id = select_backend_workspace(&target.base_url)
                .await
                .map_err(|error| io::Error::other(error.to_string()))?
                .ok_or_else(|| io::Error::other("Backend workspace picker cancelled"))?;
            target.select_workspace(workspace_id);
        }
        let mut response = list_backend_workers(&target).await.map_err(|error| {
            io::Error::other(format!(
                "failed to list Backend runtime workers from {}: {error}",
                target.base_url
            ))
        })?;
        if include_stopped {
            match list_backend_stopped_workers(&target).await {
                Ok(stopped) => {
                    response.items.extend(stopped.items);
                    response.diagnostics.extend(stopped.diagnostics);
                }
                Err(error) => response.diagnostics.push(client::BackendDiagnostic {
                    code: "backend_stopped_workers_list_failed".to_string(),
                    severity: client::BackendDiagnosticSeverity::Error,
                    message: error.to_string(),
                }),
            }
        }
        dedup_workers(&mut response.items);
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
            eprintln!(
                "Backend returned no runtime workers for workspace {} ({detail}); choose another Workspace",
                response.workspace_id
            );
            target.clear_workspace();
            continue;
        }

        let selected = match pick_worker(target.clone(), response.items)? {
            WorkerPickerResult::SwitchWorkspace => {
                target.clear_workspace();
                continue;
            }
            WorkerPickerResult::Selected(selected) => selected,
        };
        let worker = if selected.state == "stopped" {
            let restore_target = target
                .runtime_target(selected.runtime_id.clone(), selected.worker_id.clone())
                .map_err(|error| io::Error::other(error.to_string()))?;
            restore_backend_worker(&restore_target)
                .await
                .map_err(|error| {
                    io::Error::other(format!(
                        "failed to restore Backend worker {}/{}: {error}",
                        selected.runtime_id, selected.worker_id
                    ))
                })?
                .result
                .worker
                .unwrap_or(selected)
        } else {
            selected
        };
        let attach_target = target
            .runtime_target(worker.runtime_id, worker.worker_id)
            .map_err(|error| io::Error::other(error.to_string()))?;
        return console::run_backend_runtime(attach_target).await;
    }
}

fn dedup_workers(workers: &mut Vec<BackendWorkerSummary>) {
    let mut seen = std::collections::HashSet::new();
    workers.retain(|worker| seen.insert((worker.runtime_id.clone(), worker.worker_id.clone())));
}

enum WorkerPickerResult {
    Selected(BackendWorkerSummary),
    SwitchWorkspace,
}

fn pick_worker(
    target: BackendRuntimeListTarget,
    mut workers: Vec<BackendWorkerSummary>,
) -> Result<WorkerPickerResult, Box<dyn Error>> {
    workers.sort_by(|a, b| {
        a.runtime_id
            .cmp(&b.runtime_id)
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.worker_id.cmp(&b.worker_id))
    });
    workers.truncate(MAX_ROWS);

    let mut state = BackendWorkerPickerState::new(target, workers);
    with_inline_terminal(
        VIEWPORT_LINES,
        |terminal| -> Result<_, Box<dyn std::error::Error>> {
            loop {
                terminal.draw(|frame| draw(frame, &state))?;
                match poll_event()? {
                    None => continue,
                    Some(Action::Up) => state.previous(),
                    Some(Action::Down) => state.next(),
                    Some(Action::Submit) => {
                        return Ok(WorkerPickerResult::Selected(
                            state.selected_worker().clone(),
                        ));
                    }
                    Some(Action::SwitchWorkspace) => {
                        return Ok(WorkerPickerResult::SwitchWorkspace);
                    }
                    Some(Action::Cancel) => {
                        return Err(Box::new(io::Error::other(
                            "Backend worker picker cancelled",
                        )));
                    }
                }
            }
        },
    )
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
        if self.selected + 1 < self.workers.len() {
            self.selected += 1;
        }
    }

    fn previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn selected_worker(&self) -> &BackendWorkerSummary {
        &self.workers[self.selected]
    }
}

enum Action {
    Up,
    Down,
    Submit,
    SwitchWorkspace,
    Cancel,
}

fn poll_event() -> io::Result<Option<Action>> {
    if !event::poll(Duration::from_millis(100))? {
        return Ok(None);
    }
    match event::read()? {
        TermEvent::Key(k) if k.kind != KeyEventKind::Release => {
            let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
            Ok(match k.code {
                KeyCode::Up => Some(Action::Up),
                KeyCode::Down => Some(Action::Down),
                KeyCode::Char('k') if !ctrl => Some(Action::Up),
                KeyCode::Char('j') if !ctrl => Some(Action::Down),
                KeyCode::Enter => Some(Action::Submit),
                KeyCode::Char('w') if !ctrl => Some(Action::SwitchWorkspace),
                KeyCode::Esc => Some(Action::Cancel),
                KeyCode::Char('c') if ctrl => Some(Action::Cancel),
                _ => None,
            })
        }
        _ => Ok(None),
    }
}

fn draw(frame: &mut Frame<'_>, state: &BackendWorkerPickerState) {
    let area = frame.area();
    let mut constraints: Vec<Constraint> = Vec::with_capacity(state.workers.len() + 3);
    constraints.push(Constraint::Length(1));
    for _ in &state.workers {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Length(1));
    let layout = Layout::vertical(constraints).split(area);

    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            picker_title(&state.target),
            Style::default().add_modifier(Modifier::BOLD),
        )])),
        layout[0],
    );

    for (i, worker) in state.workers.iter().enumerate() {
        frame.render_widget(
            Paragraph::new(row_line(worker, i == state.selected)),
            layout[i + 1],
        );
    }

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("  "),
            Span::styled("[↑/↓]", Style::default().fg(Color::DarkGray)),
            Span::raw(" select   "),
            Span::styled("[enter]", Style::default().fg(Color::Green)),
            Span::raw(" attach   "),
            Span::styled("[w]", Style::default().fg(Color::Cyan)),
            Span::raw(" switch Workspace   "),
            Span::styled("[esc]", Style::default().fg(Color::Yellow)),
            Span::raw(" cancel"),
        ])),
        layout[state.workers.len() + 1],
    );
}

fn picker_title(target: &BackendRuntimeListTarget) -> String {
    let workspace = target
        .workspace_id
        .as_deref()
        .map(short_text)
        .unwrap_or_else(|| "unscoped".to_string());
    let runtime = target
        .runtime_id
        .as_deref()
        .map(short_text)
        .unwrap_or_else(|| "all runtimes".to_string());
    format!("backend workers   workspace: {workspace}   runtime: {runtime}")
}

fn row_line(worker: &BackendWorkerSummary, selected: bool) -> Line<'static> {
    let marker = if selected { "▶ " } else { "  " };
    let id_style = if selected {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    };
    let preview_style = if selected {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let label = if worker.label.is_empty() {
        worker.worker_id.as_str()
    } else {
        worker.label.as_str()
    };
    let profile = worker.profile.as_deref().unwrap_or("-");

    Line::from(vec![
        Span::raw(marker),
        Span::styled(short_worker_id(worker), id_style),
        Span::raw("  "),
        Span::styled(
            format!("[{}]", worker.state),
            state_style(worker.state.as_str()),
        ),
        Span::raw("  "),
        Span::styled(
            format!("profile:{profile}"),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("  "),
        Span::styled(
            working_directory_text(worker),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("  "),
        Span::styled(label.to_string(), preview_style),
    ])
}

fn state_style(state: &str) -> Style {
    match state {
        "running" | "idle" | "active" => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        "stopped" | "complete" => Style::default().fg(Color::Yellow),
        "failed" | "error" => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        _ => Style::default().fg(Color::Yellow),
    }
}

fn short_text(text: &str) -> String {
    const MAX: usize = 24;
    let mut chars = text.chars();
    let shortened: String = chars.by_ref().take(MAX).collect();
    if chars.next().is_some() {
        format!("{shortened}…")
    } else {
        shortened
    }
}

fn short_worker_id(worker: &BackendWorkerSummary) -> String {
    worker.resource_key.clone()
}

fn working_directory_text(worker: &BackendWorkerSummary) -> String {
    let Some(wd) = worker.working_directory.as_ref() else {
        return "wd:—".to_string();
    };
    let cleanliness = wd.cleanliness.as_deref().unwrap_or("unknown");
    format!(
        "wd:{}:{} {} {}",
        wd.repository_key, wd.working_directory_id, wd.status, cleanliness
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
            resource_key: "W-1".to_string(),
            host_id: "host".to_string(),
            label: "label".to_string(),
            display_name: "label".to_string(),
            singleton_key: None,
            tags: Vec::new(),
            profile: profile.map(str::to_string),
            workspace: BackendWorkerWorkspaceSummary {
                visibility: "workspace".to_string(),
                identity: "ws".to_string(),
                workspace_id: Some("ws".to_string()),
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
    fn worker_row_matches_inline_picker_shape() {
        let row = row_line(&worker("runtime-a", "worker-b", Some("default")), true);
        let text = row
            .spans
            .into_iter()
            .map(|span| span.content)
            .collect::<String>();
        assert!(text.starts_with("▶ W-1"));
        assert!(text.contains("[running]"));
        assert!(text.contains("profile:default"));
        assert!(text.contains("wd:—"));
    }

    #[test]
    fn picker_title_uses_backend_worker_wording() {
        let target = BackendRuntimeListTarget::new(
            "http://127.0.0.1:8787",
            Some("workspace-abcdef".to_string()),
            None,
        );
        assert_eq!(
            picker_title(&target),
            "backend workers   workspace: workspace-abcdef   runtime: all runtimes"
        );
    }
}
