use std::io;
use std::time::Duration;

use client::{StandaloneWorkerListIntent, StandaloneWorkerResumeIntent, Target};
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::prelude::{Color, Line, Modifier, Span, Style};
use ratatui::widgets::Paragraph;
use ratatui::{TerminalOptions, Viewport};
use standalone::{StandaloneListScope, StandaloneWorkerRecord, StandaloneWorkerStore};
use thiserror::Error;

const LIMIT: usize = 100;

pub(crate) fn pick(
    target: &dyn Target,
    include_all: bool,
) -> Result<Option<StandaloneWorkerResumeIntent>, StandalonePickerError> {
    let intent = target
        .standalone_worker_list(include_all)
        .map_err(StandalonePickerError::Target)?;
    let records = load_records(&intent)?;
    if records.is_empty() {
        return Err(StandalonePickerError::NoWorkers { include_all });
    }
    let selected = run_picker(records)?;
    selected
        .map(|record| {
            target
                .standalone_worker_resume(record.worker_id.to_string())
                .map_err(StandalonePickerError::Target)
        })
        .transpose()
}

fn load_records(
    intent: &StandaloneWorkerListIntent,
) -> Result<Vec<StandaloneWorkerRecord>, StandalonePickerError> {
    let store = StandaloneWorkerStore::open(&intent.state_dir)
        .map_err(StandalonePickerError::StateStore)?;
    store
        .list(
            &intent.cwd,
            if intent.include_all {
                StandaloneListScope::All
            } else {
                StandaloneListScope::CurrentCwd
            },
            LIMIT,
        )
        .map_err(StandalonePickerError::StateStore)
}

fn run_picker(
    records: Vec<StandaloneWorkerRecord>,
) -> Result<Option<StandaloneWorkerRecord>, StandalonePickerError> {
    let height = u16::try_from(records.len().saturating_add(3).min(20)).unwrap_or(20);
    let mut terminal = Terminal::with_options(
        CrosstermBackend::new(io::stdout()),
        TerminalOptions {
            viewport: Viewport::Inline(height),
        },
    )
    .map_err(StandalonePickerError::Io)?;
    let mut selected = 0usize;
    loop {
        terminal
            .draw(|frame| draw(frame, &records, selected))
            .map_err(StandalonePickerError::Io)?;
        if !event::poll(Duration::from_millis(100)).map_err(StandalonePickerError::Io)? {
            continue;
        }
        let TermEvent::Key(key) = event::read().map_err(StandalonePickerError::Io)? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if !ctrl => {
                selected = selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') if !ctrl => {
                selected = (selected + 1).min(records.len() - 1);
            }
            KeyCode::Enter => return Ok(Some(records[selected].clone())),
            KeyCode::Esc => return Ok(None),
            KeyCode::Char('c') if ctrl => return Ok(None),
            _ => {}
        }
    }
}

fn draw(frame: &mut ratatui::Frame<'_>, records: &[StandaloneWorkerRecord], selected: usize) {
    let mut constraints = vec![Constraint::Length(1)];
    constraints.extend(records.iter().map(|_| Constraint::Length(1)));
    constraints.push(Constraint::Length(1));
    let rows = Layout::vertical(constraints).split(frame.area());
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "resume standalone Worker",
            Style::default().add_modifier(Modifier::BOLD),
        ))),
        rows[0],
    );
    for (index, record) in records.iter().enumerate() {
        let active = index == selected;
        let marker = if active { "▶ " } else { "  " };
        let style = if active {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let cwd = record.cwd.canonical_path.display();
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(marker),
                Span::styled(
                    format!("{} ({})", record.worker_name, record.worker_id.short()),
                    style,
                ),
                Span::raw(format!(
                    "  [{:?}]  updated:{}  {}",
                    record.status, record.updated_at_unix_ms, cwd
                )),
            ])),
            rows[index + 1],
        );
    }
    frame.render_widget(
        Paragraph::new("  [↑/↓] select   [enter] restore   [esc] cancel"),
        rows[records.len() + 1],
    );
}

#[derive(Debug, Error)]
pub(crate) enum StandalonePickerError {
    #[error("standalone target error: {0}")]
    Target(#[source] client::TargetError),
    #[error("standalone Worker state is unavailable: {0}")]
    StateStore(#[source] standalone::StandaloneStoreError),
    #[error(
        "no standalone Workers found for this cwd; use `yoi --local --resume --all` to include all cwd identities"
    )]
    NoWorkers { include_all: bool },
    #[error("standalone Worker picker I/O failed: {0}")]
    Io(#[source] io::Error),
}

#[cfg(test)]
mod tests {
    use client::StandaloneTarget;

    use super::*;

    #[test]
    fn empty_picker_keeps_current_cwd_as_default_scope() {
        let temp = tempfile::tempdir().expect("tempdir");
        let target = StandaloneTarget::new(temp.path());
        let error = pick(&target, false).expect_err("empty picker should fail explicitly");
        assert!(error.to_string().contains("this cwd"));
        assert!(error.to_string().contains("--all"));
    }
}
