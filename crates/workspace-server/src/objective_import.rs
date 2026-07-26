use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use project_record::validate_record_id;
use serde::Deserialize;

use crate::store::{
    ControlPlaneStore, MemoryStagingRecord, ObjectiveRecord, ObjectiveResourceRecord,
    ObjectiveTicketLinkRecord,
};
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectiveImportReport {
    pub objectives_imported: usize,
    pub objective_resources_imported: usize,
    pub objective_ticket_links_imported: usize,
    pub objective_ticket_links_skipped: usize,
    pub memory_staging_records_imported: usize,
    pub invalid_records: Vec<String>,
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

pub fn import_legacy_objectives_and_memory_staging<S: ControlPlaneStore>(
    workspace_root: &Path,
    workspace_id: &str,
    store: &S,
) -> Result<ObjectiveImportReport> {
    let mut report = ObjectiveImportReport {
        objectives_imported: 0,
        objective_resources_imported: 0,
        objective_ticket_links_imported: 0,
        objective_ticket_links_skipped: 0,
        memory_staging_records_imported: 0,
        invalid_records: Vec::new(),
    };

    import_objectives(workspace_root, workspace_id, store, &mut report)?;
    import_memory_staging(workspace_root, workspace_id, store, &mut report)?;
    Ok(report)
}

fn import_objectives<S: ControlPlaneStore>(
    workspace_root: &Path,
    workspace_id: &str,
    store: &S,
    report: &mut ObjectiveImportReport,
) -> Result<()> {
    let root = workspace_root.join(".yoi/objectives");
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let objective_id = entry.file_name().to_string_lossy().to_string();
        if let Err(err) = validate_record_id(&objective_id) {
            report
                .invalid_records
                .push(format!("{objective_id}: invalid objective id: {err}"));
            continue;
        }
        match import_one_objective(&path, workspace_id, &objective_id, store) {
            Ok(item) => {
                report.objectives_imported += 1;
                report.objective_resources_imported += item.resources;
                report.objective_ticket_links_imported += item.links;
                report.objective_ticket_links_skipped += item.skipped_links;
            }
            Err(err) => report
                .invalid_records
                .push(format!("{objective_id}: {err}")),
        }
    }
    Ok(())
}

struct ImportedObjectiveCounts {
    resources: usize,
    links: usize,
    skipped_links: usize,
}

fn import_one_objective<S: ControlPlaneStore>(
    objective_dir: &Path,
    workspace_id: &str,
    objective_id: &str,
    store: &S,
) -> Result<ImportedObjectiveCounts> {
    let item_path = objective_dir.join("item.md");
    let raw = fs::read_to_string(&item_path)?;
    let (frontmatter, body) = split_frontmatter(&raw, objective_id)?;
    let meta: ObjectiveFrontmatter = serde_yaml::from_str(frontmatter)?;
    let now = Utc::now().to_rfc3339();
    let created_at = meta.created_at.clone().unwrap_or_else(|| now.clone());
    let updated_at = meta.updated_at.clone().unwrap_or_else(|| now.clone());

    store.upsert_objective(&ObjectiveRecord {
        workspace_id: workspace_id.to_string(),
        objective_id: objective_id.to_string(),
        title: meta.title,
        state: meta.state,
        body_md: body.trim_start().to_string(),
        created_at: created_at.clone(),
        updated_at: updated_at.clone(),
    })?;

    let mut links = Vec::new();
    let mut skipped_links = 0;
    for ticket_id in meta.linked_tickets {
        if validate_record_id(&ticket_id).is_ok() {
            links.push(ObjectiveTicketLinkRecord {
                workspace_id: workspace_id.to_string(),
                objective_id: objective_id.to_string(),
                ticket_id,
                kind: "linked".to_string(),
                created_at: now.clone(),
            });
        } else {
            skipped_links += 1;
        }
    }
    let imported_links = links.len();
    let links_imported = if let Err(err) =
        store.replace_objective_ticket_links(workspace_id, objective_id, &links)
    {
        // The legacy filesystem allowed links to tickets that may no longer
        // exist. Preserve the Objective itself and make the skipped count clear
        // rather than failing the whole import on a foreign-key mismatch.
        skipped_links += imported_links;
        eprintln!(
            "warning: skipped {imported_links} ticket link(s) for objective {objective_id}: {err}"
        );
        0
    } else {
        imported_links
    };

    let mut resources = 0;
    for file in walk_files(objective_dir)? {
        let relative = file
            .strip_prefix(objective_dir)
            .map_err(|err| Error::Store(err.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        if relative == "item.md" || relative.starts_with("_staging/") {
            continue;
        }
        let Ok(body) = fs::read_to_string(&file) else {
            continue;
        };
        store.upsert_objective_resource(&ObjectiveResourceRecord {
            workspace_id: workspace_id.to_string(),
            objective_id: objective_id.to_string(),
            resource_path: relative,
            body,
            media_type: Some("text/markdown".to_string()),
            created_at: created_at.clone(),
            updated_at: updated_at.clone(),
        })?;
        resources += 1;
    }

    Ok(ImportedObjectiveCounts {
        resources,
        links: links_imported,
        skipped_links,
    })
}

fn import_memory_staging<S: ControlPlaneStore>(
    workspace_root: &Path,
    workspace_id: &str,
    store: &S,
    report: &mut ObjectiveImportReport,
) -> Result<()> {
    let root = workspace_root.join(".yoi/memory/_staging");
    if !root.exists() {
        return Ok(());
    }
    let now = Utc::now().to_rfc3339();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let raw_json = fs::read_to_string(&path)?;
        let candidate_id = candidate_id_from_json(&raw_json)
            .or_else(|| {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().to_string())
            })
            .ok_or_else(|| Error::Store(format!("missing candidate id: {}", path.display())))?;
        store.upsert_memory_staging_record(&MemoryStagingRecord {
            workspace_id: workspace_id.to_string(),
            candidate_id,
            raw_json,
            source_path: Some(path.to_string_lossy().to_string()),
            imported_at: now.clone(),
        })?;
        report.memory_staging_records_imported += 1;
    }
    Ok(())
}

fn candidate_id_from_json(raw_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(raw_json)
        .ok()
        .and_then(|value| {
            value
                .get("id")
                .and_then(|id| id.as_str())
                .map(str::to_string)
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

fn walk_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    walk_files_inner(root, &mut files)?;
    Ok(files)
}

fn walk_files_inner(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_files_inner(&path, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}
