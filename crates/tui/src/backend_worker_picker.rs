use std::error::Error;
use std::io;
use std::time::Duration;

use client::{
    BackendRuntimeListTarget, BackendRuntimeTarget, BackendWorkerSummary, list_backend_workers,
};
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};

use crate::console;

const MAX_ROWS: usize = 10;
const VIEWPORT_LINES: u16 = MAX_ROWS as u16 + 4;

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
    workers.truncate(MAX_ROWS);

    let mut state = BackendWorkerPickerState::new(target, workers);
    let mut terminal = make_inline_terminal()?;
    loop {
        terminal.draw(|frame| draw(frame, &state))?;
        match poll_event()? {
            None => continue,
            Some(Action::Up) => state.previous(),
            Some(Action::Down) => state.next(),
            Some(Action::Submit) => {
                close_viewport(&mut terminal)?;
                return Ok(state.selected_worker().clone());
            }
            Some(Action::Cancel) => {
                close_viewport(&mut terminal)?;
                return Err(Box::new(io::Error::other(
                    "Backend worker picker cancelled",
                )));
            }
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

fn make_inline_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    let backend = CrosstermBackend::new(io::stdout());
    Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(VIEWPORT_LINES),
        },
    )
}

fn close_viewport(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    let area = terminal.get_frame().area();
    let last_row = area.bottom().saturating_sub(1);
    terminal.set_cursor_position((0, last_row))?;
    use std::io::Write;
    let mut out = io::stdout();
    out.write_all(b"\r\n")?;
    out.flush()?;
    Ok(())
}

enum Action {
    Up,
    Down,
    Submit,
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

fn short_worker_id(worker: &BackendWorkerSummary) -> String {
    format!(
        "{}:{}",
        short_text(&worker.runtime_id),
        short_text(&worker.worker_id)
    )
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

fn working_directory_text(worker: &BackendWorkerSummary) -> String {
    let Some(wd) = worker.working_directory.as_ref() else {
        return "wd:—".to_string();
    };
    let cleanliness = wd.cleanliness.as_deref().unwrap_or("unknown");
    format!(
        "wd:{}:{} {} {}",
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
    fn worker_row_matches_inline_picker_shape() {
        let row = row_line(&worker("runtime-a", "worker-b", Some("default")), true);
        let text = row
            .spans
            .into_iter()
            .map(|span| span.content)
            .collect::<String>();
        assert!(text.starts_with("▶ runtime-a:worker-b"));
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
