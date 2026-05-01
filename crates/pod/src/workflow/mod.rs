//! Pod-side Workflow resolver.
//!
//! Turns `Segment::WorkflowInvoke { slug }` into system-message attachments:
//! dependency Knowledge bodies first, then the Workflow body. Resolution is
//! strict for explicit user invocations: missing workflows, non-user-invocable
//! workflows, and missing Knowledge requirements are returned as errors before
//! the turn is handed to the Worker.

use std::fmt;

use llm_worker::Item;
use memory::schema::split_frontmatter;
use memory::{Slug, WorkflowRegistry, WorkspaceLayout};

#[derive(Debug)]
pub enum WorkflowResolveError {
    InvalidSlug(memory::LintError),
    NotFound {
        slug: String,
    },
    NotUserInvocable {
        slug: String,
    },
    KnowledgeNotFound {
        workflow: String,
        slug: String,
    },
    KnowledgeRead {
        workflow: String,
        slug: String,
        source: std::io::Error,
    },
    KnowledgeFrontmatter {
        workflow: String,
        slug: String,
        source: memory::LintError,
    },
}

impl fmt::Display for WorkflowResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSlug(e) => write!(f, "invalid workflow slug: {e}"),
            Self::NotFound { slug } => write!(f, "workflow /{slug} is not registered"),
            Self::NotUserInvocable { slug } => {
                write!(f, "workflow /{slug} is not user-invocable")
            }
            Self::KnowledgeNotFound { workflow, slug } => write!(
                f,
                "workflow /{workflow} requires missing Knowledge slug `{slug}`"
            ),
            Self::KnowledgeRead {
                workflow,
                slug,
                source,
            } => write!(
                f,
                "workflow /{workflow} could not read required Knowledge `{slug}`: {source}"
            ),
            Self::KnowledgeFrontmatter {
                workflow,
                slug,
                source,
            } => write!(
                f,
                "workflow /{workflow} required Knowledge `{slug}` has invalid frontmatter: {source}"
            ),
        }
    }
}

impl std::error::Error for WorkflowResolveError {}

pub fn resolve_workflow_invocation(
    registry: &WorkflowRegistry,
    layout: &WorkspaceLayout,
    raw_slug: &str,
) -> Result<Vec<Item>, WorkflowResolveError> {
    let slug = Slug::parse(raw_slug.to_string()).map_err(WorkflowResolveError::InvalidSlug)?;
    let record = registry
        .get(&slug)
        .ok_or_else(|| WorkflowResolveError::NotFound {
            slug: raw_slug.to_string(),
        })?;
    if !record.user_invocable {
        return Err(WorkflowResolveError::NotUserInvocable {
            slug: raw_slug.to_string(),
        });
    }

    let mut out = Vec::new();
    for req in &record.requires {
        let path = layout.knowledge_path(req);
        let raw = std::fs::read_to_string(&path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                WorkflowResolveError::KnowledgeNotFound {
                    workflow: slug.to_string(),
                    slug: req.to_string(),
                }
            } else {
                WorkflowResolveError::KnowledgeRead {
                    workflow: slug.to_string(),
                    slug: req.to_string(),
                    source,
                }
            }
        })?;
        let (_yaml, body) = split_frontmatter(&raw).map_err(|source| {
            WorkflowResolveError::KnowledgeFrontmatter {
                workflow: slug.to_string(),
                slug: req.to_string(),
                source,
            }
        })?;
        out.push(Item::system_message(format!(
            "[Workflow /{} requires Knowledge #{}]\n{}",
            slug,
            req,
            body.trim_end()
        )));
    }
    out.push(Item::system_message(format!(
        "[Workflow /{}]\n{}",
        slug,
        record.body.trim_end()
    )));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(path: &std::path::Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn setup() -> (TempDir, WorkspaceLayout, WorkflowRegistry) {
        let dir = TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        write(
            &dir.path().join(".insomnia/knowledge/policy.md"),
            "---\ncreated_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\nkind: policy\ndescription: p\nmodel_invokation: false\nuser_invocable: true\nlast_sources: []\n---\npolicy body\n",
        );
        write(
            &dir.path().join(".insomnia/memory/workflow/run-it.md"),
            "---\ndescription: run\nrequires: [policy]\n---\nworkflow body\n",
        );
        let registry = memory::load_workflows(&layout).unwrap();
        (dir, layout, registry)
    }

    #[test]
    fn resolves_requires_before_workflow_body() {
        let (_dir, layout, registry) = setup();
        let items = resolve_workflow_invocation(&registry, &layout, "run-it").unwrap();
        assert_eq!(items.len(), 2);
        let first = format!("{:?}", items[0]);
        let second = format!("{:?}", items[1]);
        assert!(first.contains("Knowledge #policy"));
        assert!(first.contains("policy body"));
        assert!(second.contains("[Workflow /run-it]"));
        assert!(second.contains("workflow body"));
    }

    #[test]
    fn user_invocable_false_errors() {
        let (dir, layout, _registry) = setup();
        write(
            &dir.path().join(".insomnia/memory/workflow/hidden.md"),
            "---\ndescription: hidden\nuser_invocable: false\n---\nbody\n",
        );
        let registry = memory::load_workflows(&layout).unwrap();
        let err = resolve_workflow_invocation(&registry, &layout, "hidden").unwrap_err();
        assert!(matches!(err, WorkflowResolveError::NotUserInvocable { .. }));
    }

    #[test]
    fn missing_required_knowledge_errors() {
        let dir = TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        write(
            &dir.path().join(".insomnia/memory/workflow/bad.md"),
            "---\ndescription: bad\nrequires: [ghost]\n---\nbody\n",
        );
        let registry = memory::load_workflows(&layout).unwrap();
        let err = resolve_workflow_invocation(&registry, &layout, "bad").unwrap_err();
        assert!(matches!(
            err,
            WorkflowResolveError::KnowledgeNotFound { .. }
        ));
    }
}
