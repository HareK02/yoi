use std::io;
use std::time::Duration;

use client::{StandaloneWorkerListIntent, StandaloneWorkerResumeIntent, Target};
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::prelude::{Color, Line, Modifier, Span, Style};
use ratatui::widgets::Paragraph;
use standalone::{StandaloneListScope, StandaloneWorkerListEntry, StandaloneWorkerStore};
use thiserror::Error;

use crate::inline_terminal::with_inline_terminal;

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
        .map(|record| resume_selected(target, record))
        .transpose()
}

fn resume_selected(
    target: &dyn Target,
    record: StandaloneWorkerListEntry,
) -> Result<StandaloneWorkerResumeIntent, StandalonePickerError> {
    if let Some(reason) = record.restore_blocker {
        return Err(StandalonePickerError::StateStore(
            standalone::StandaloneStoreError::WorkspaceMemoryRetirement {
                id: record.worker_id,
                reason,
            },
        ));
    }
    target
        .standalone_worker_resume(record.worker_id.to_string())
        .map_err(StandalonePickerError::Target)
}

fn load_records(
    intent: &StandaloneWorkerListIntent,
) -> Result<Vec<StandaloneWorkerListEntry>, StandalonePickerError> {
    let store = match StandaloneWorkerStore::open_existing(&intent.state_dir) {
        Ok(store) => store,
        Err(standalone::StandaloneStoreError::Io(error))
            if error.kind() == io::ErrorKind::NotFound =>
        {
            return Ok(Vec::new());
        }
        Err(error) => return Err(StandalonePickerError::StateStore(error)),
    };
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
    records: Vec<StandaloneWorkerListEntry>,
) -> Result<Option<StandaloneWorkerListEntry>, StandalonePickerError> {
    let height = u16::try_from(records.len().saturating_add(3).min(20)).unwrap_or(20);
    with_inline_terminal(height, |terminal| {
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
    })
}

fn draw(frame: &mut ratatui::Frame<'_>, records: &[StandaloneWorkerListEntry], selected: usize) {
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
                    format!(
                        "{} ({}){}",
                        record.worker_name,
                        record.worker_id.short(),
                        if record.restore_blocker.is_some() {
                            " [unavailable]"
                        } else {
                            ""
                        }
                    ),
                    style,
                ),
                Span::raw(format!(
                    "  [{:?}]  updated:{}  {}",
                    record.status, record.updated_at_unix_ms, cwd
                )),
                Span::styled(
                    record
                        .restore_blocker
                        .map(|reason| format!("  [unavailable: {reason}]"))
                        .unwrap_or_default(),
                    Style::default().fg(Color::Yellow),
                ),
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
        "no standalone Workers found for this cwd; use `yoi --local resume --all` to include all cwd identities"
    )]
    NoWorkers { include_all: bool },
    #[error("standalone Worker picker I/O failed: {0}")]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use client::StandaloneTarget;

    use super::*;

    #[test]
    fn legacy_discovery_selection_keeps_exact_restore_identity_without_execution() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workers");
        let store = StandaloneWorkerStore::open(&root).unwrap();
        let allocation = store
            .allocate(temp.path(), standalone::StaleLeasePolicy::Reject)
            .unwrap();
        let manifest = manifest::WorkerManifest::from_toml(
            r#"
[worker]
name = "legacy-picker"
[model]
scheme = "anthropic"
model_id = "fixture-only"
[engine]
[scope]
allow = []
"#,
        )
        .unwrap();
        let record = store
            .commit_created(
                &allocation,
                manifest,
                "legacy-picker".into(),
                session_store::new_session_id(),
                None,
            )
            .unwrap();
        let path = root.join(record.worker_id.to_string()).join("record.json");
        let mut raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        raw["schema_version"] = serde_json::json!(2);
        raw["manifest"]["manifest"]["feature"]["memory"] =
            serde_json::json!({"profile":{"enabled":false}});
        let before = serde_json::to_vec(&raw).unwrap();
        std::fs::write(&path, &before).unwrap();
        let target = StandaloneTarget::new(&root);
        let listed = load_records(&StandaloneWorkerListIntent {
            state_dir: root.clone(),
            cwd: temp.path().to_path_buf(),
            include_all: false,
        })
        .unwrap();
        assert_eq!(listed.len(), 1);
        let intent = resume_selected(&target, listed[0].clone()).unwrap();
        assert_eq!(intent.worker_id, record.worker_id.to_string());
        assert_eq!(intent.state_dir, root);
        assert!(
            store.load(record.worker_id).is_err(),
            "selection must not bypass strict execution decoding"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!path.parent().unwrap().join("commit.pending").exists());
        raw["manifest"]["manifest"]["feature"]["memory"]["profile"]["enabled"] =
            serde_json::json!(true);
        let blocked_bytes = serde_json::to_vec(&raw).unwrap();
        std::fs::write(&path, &blocked_bytes).unwrap();
        let blocked = load_records(&StandaloneWorkerListIntent {
            state_dir: root,
            cwd: temp.path().to_path_buf(),
            include_all: false,
        })
        .unwrap()
        .remove(0);
        assert!(blocked.restore_blocker.is_some());
        assert!(matches!(
            resume_selected(&target, blocked),
            Err(StandalonePickerError::StateStore(
                standalone::StandaloneStoreError::WorkspaceMemoryRetirement { .. }
            ))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), blocked_bytes);
    }

    #[test]
    fn empty_picker_keeps_current_cwd_as_default_scope() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state_dir = temp.path().join("missing-store");
        let target = StandaloneTarget::new(&state_dir);
        let error = pick(&target, false).expect_err("empty picker should fail explicitly");
        assert!(
            !state_dir.exists(),
            "observational discovery must not create storage"
        );
        assert!(error.to_string().contains("this cwd"));
        assert!(error.to_string().contains("--all"));
    }
}
