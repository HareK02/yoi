use std::fs;
use std::path::{Path, PathBuf};

use project_record::validate_record_id;
use serde::{Deserialize, Serialize};
use ticket::config::TicketConfig;
use ticket::{LocalTicketBackend, TicketIdOrSlug, TicketListQuery};

use crate::{Error, Result};

const DETAIL_BODY_LIMIT: usize = 64 * 1024;
const SUMMARY_BODY_LIMIT: usize = 240;

#[derive(Debug, Clone)]
pub struct LocalProjectRecordReader {
    workspace_root: PathBuf,
    ticket_backend: LocalTicketBackend,
}

impl LocalProjectRecordReader {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Result<Self> {
        let workspace_root = workspace_root.into();
        let ticket_config = TicketConfig::load_workspace(&workspace_root)
            .map_err(|error| Error::Config(format!("load Ticket workspace settings: {error}")))?;
        let ticket_backend = LocalTicketBackend::new(ticket_config.backend_root().to_path_buf())
            .with_record_language(ticket_config.ticket_record_language());
        Ok(Self {
            workspace_root,
            ticket_backend,
        })
    }

    pub fn workspace_root(&self) -> &Path {
        self.workspace_root.as_path()
    }

    pub fn list_tickets(&self, limit: usize) -> Result<ProjectRecordList<TicketSummary>> {
        let partial = self.ticket_backend.list_partial(TicketListQuery::all())?;
        let mut items = partial
            .tickets
            .into_iter()
            .map(|item| TicketSummary {
                id: item.id,
                title: item.title,
                state: item.workflow_state.as_str().to_string(),
                priority: item.priority,
                updated_at: item.updated_at,
                queued_by: item.queued_by,
                queued_at: item.queued_at,
                record_source: "local_yoi_ticket".to_string(),
            })
            .collect::<Vec<_>>();
        items.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        items.truncate(limit.min(200));
        Ok(ProjectRecordList {
            items,
            invalid_records: partial
                .invalid_records
                .into_iter()
                .map(|record| InvalidProjectRecord {
                    label: record.label,
                    reason: record.reason,
                })
                .collect(),
            record_authority: "local_yoi_project_records".to_string(),
        })
    }

    pub fn ticket(&self, id: &str) -> Result<TicketDetail> {
        validate_project_id(id)?;
        let partial = self
            .ticket_backend
            .show_partial(TicketIdOrSlug::Id(id.to_string()))?;
        let ticket = partial.ticket;
        let (body, body_truncated) =
            truncate_body(ticket.document.body.as_str(), DETAIL_BODY_LIMIT);
        Ok(TicketDetail {
            id: ticket.meta.id,
            title: ticket.meta.title,
            state: ticket.meta.workflow_state.as_str().to_string(),
            priority: ticket.meta.priority,
            created_at: ticket.meta.created_at,
            updated_at: ticket.meta.updated_at,
            queued_by: ticket.meta.queued_by,
            queued_at: ticket.meta.queued_at,
            risk_flags: ticket.meta.risk_flags,
            body,
            body_truncated,
            event_count: ticket.events.len(),
            artifact_count: ticket.artifacts.len(),
            record_source: "local_yoi_ticket".to_string(),
        })
    }

    pub fn list_objectives(&self, limit: usize) -> Result<ProjectRecordList<ObjectiveSummary>> {
        let mut items = Vec::new();
        let mut invalid_records = Vec::new();
        let root = self.workspace_root.join(".yoi/objectives");
        if !root.exists() {
            return Ok(ProjectRecordList {
                items,
                invalid_records,
                record_authority: "local_yoi_project_records".to_string(),
            });
        }

        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().to_string();
            match read_objective_summary(&path, &id) {
                Ok(item) => items.push(item),
                Err(error) => invalid_records.push(InvalidProjectRecord {
                    label: id,
                    reason: error.to_string(),
                }),
            }
        }
        items.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        items.truncate(limit.min(200));
        Ok(ProjectRecordList {
            items,
            invalid_records,
            record_authority: "local_yoi_project_records".to_string(),
        })
    }

    pub fn objective(&self, id: &str) -> Result<ObjectiveDetail> {
        validate_project_id(id)?;
        let path = self.workspace_root.join(".yoi/objectives").join(id);
        let raw = fs::read_to_string(path.join("item.md"))?;
        let (frontmatter, body) = split_frontmatter(&raw, id)?;
        let meta: ObjectiveFrontmatter = serde_yaml::from_str(frontmatter)?;
        let (body, body_truncated) = truncate_body(body, DETAIL_BODY_LIMIT);
        Ok(ObjectiveDetail {
            id: id.to_string(),
            title: meta.title,
            state: meta.state,
            created_at: meta.created_at,
            updated_at: meta.updated_at,
            linked_tickets: meta.linked_tickets,
            body,
            body_truncated,
            record_source: "local_yoi_objective".to_string(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectRecordList<T> {
    pub items: Vec<T>,
    pub invalid_records: Vec<InvalidProjectRecord>,
    pub record_authority: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InvalidProjectRecord {
    pub label: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TicketSummary {
    pub id: String,
    pub title: String,
    pub state: String,
    pub priority: String,
    pub updated_at: Option<String>,
    pub queued_by: Option<String>,
    pub queued_at: Option<String>,
    pub record_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TicketDetail {
    pub id: String,
    pub title: String,
    pub state: String,
    pub priority: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub queued_by: Option<String>,
    pub queued_at: Option<String>,
    pub risk_flags: Vec<String>,
    pub body: String,
    pub body_truncated: bool,
    pub event_count: usize,
    pub artifact_count: usize,
    pub record_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveSummary {
    pub id: String,
    pub title: String,
    pub state: String,
    pub updated_at: Option<String>,
    pub summary: String,
    pub linked_tickets: Vec<String>,
    pub record_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveDetail {
    pub id: String,
    pub title: String,
    pub state: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub linked_tickets: Vec<String>,
    pub body: String,
    pub body_truncated: bool,
    pub record_source: String,
}

#[derive(Debug, Deserialize)]
struct ObjectiveFrontmatter {
    title: String,
    state: String,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    linked_tickets: Vec<String>,
}

fn read_objective_summary(path: &Path, id: &str) -> Result<ObjectiveSummary> {
    validate_project_id(id)?;
    let raw = fs::read_to_string(path.join("item.md"))?;
    let (frontmatter, body) = split_frontmatter(&raw, id)?;
    let meta: ObjectiveFrontmatter = serde_yaml::from_str(frontmatter)?;
    Ok(ObjectiveSummary {
        id: id.to_string(),
        title: meta.title,
        state: meta.state,
        updated_at: meta.updated_at,
        summary: summarize_body(body),
        linked_tickets: meta.linked_tickets,
        record_source: "local_yoi_objective".to_string(),
    })
}

fn split_frontmatter<'a>(raw: &'a str, label: &str) -> Result<(&'a str, &'a str)> {
    let rest = raw
        .strip_prefix("---\n")
        .ok_or_else(|| Error::MissingFrontmatter(label.to_string()))?;
    let Some((frontmatter, body)) = rest.split_once("\n---\n") else {
        return Err(Error::MissingFrontmatter(label.to_string()));
    };
    Ok((frontmatter, body))
}

fn validate_project_id(id: &str) -> Result<()> {
    validate_record_id(id).map_err(|_| Error::InvalidRecordId(id.to_string()))
}

fn summarize_body(body: &str) -> String {
    let summary = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or_default();
    let (summary, truncated) = truncate_body(summary, SUMMARY_BODY_LIMIT);
    if truncated {
        format!("{summary}…")
    } else {
        summary
    }
}

fn truncate_body(body: &str, limit: usize) -> (String, bool) {
    if body.len() <= limit {
        return (body.to_string(), false);
    }
    let mut end = limit;
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    (body[..end].to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_local_yoi_ticket_and_objective_records_without_migration() {
        let dir = tempfile::tempdir().unwrap();
        write_ticket(dir.path(), "00000000001J2", "Read bridge", "ready");
        write_objective(dir.path(), "00000000001J3", "Control plane", "active");

        let reader = LocalProjectRecordReader::new(dir.path()).unwrap();
        let tickets = reader.list_tickets(20).unwrap();
        assert_eq!(tickets.record_authority, "local_yoi_project_records");
        assert_eq!(tickets.items[0].id, "00000000001J2");
        assert_eq!(tickets.items[0].state, "ready");

        let ticket = reader.ticket("00000000001J2").unwrap();
        assert!(ticket.body.contains("Ticket body"));

        let objectives = reader.list_objectives(20).unwrap();
        assert_eq!(objectives.items[0].id, "00000000001J3");
        assert_eq!(objectives.items[0].linked_tickets, vec!["00000000001J2"]);

        let objective = reader.objective("00000000001J3").unwrap();
        assert!(objective.body.contains("Objective body"));
    }
    #[test]
    fn reads_tickets_from_workspace_settings_backend_root() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".yoi")).unwrap();
        fs::write(
            dir.path().join(".yoi/workspace.toml"),
            r#"
[ticket]
language = "Japanese"

[ticket.backend]
provider = "builtin:yoi_local"
root = "project-records/tickets"
"#,
        )
        .unwrap();
        write_ticket_at(
            &dir.path().join("project-records/tickets"),
            "00000000001J4",
            "Configured root",
            "ready",
        );
        write_ticket(dir.path(), "00000000001J5", "Default root", "ready");

        let reader = LocalProjectRecordReader::new(dir.path()).unwrap();
        let tickets = reader.list_tickets(20).unwrap();
        assert_eq!(tickets.items.len(), 1);
        assert_eq!(tickets.items[0].id, "00000000001J4");
    }
    fn write_ticket(root: &Path, id: &str, title: &str, state: &str) {
        write_ticket_at(&root.join(".yoi/tickets"), id, title, state);
    }

    fn write_ticket_at(ticket_root: &Path, id: &str, title: &str, state: &str) {
        let ticket_dir = ticket_root.join(id);
        fs::create_dir_all(&ticket_dir).unwrap();
        fs::write(
            ticket_dir.join("item.md"),
            format!(
                r#"---
title: "{title}"
state: "{state}"
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-02T00:00:00Z"
---

Ticket body.
"#,
            ),
        )
        .unwrap();
        fs::write(ticket_dir.join("thread.md"), "").unwrap();
    }

    fn write_objective(root: &Path, id: &str, title: &str, state: &str) {
        let objective_dir = root.join(".yoi/objectives").join(id);
        fs::create_dir_all(&objective_dir).unwrap();
        fs::write(
            objective_dir.join("item.md"),
            format!(
                r#"---
title: "{title}"
state: "{state}"
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-02T00:00:00Z"
linked_tickets: ["00000000001J2"]
---

Objective body.
"#,
            ),
        )
        .unwrap();
    }
}
