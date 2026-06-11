//! Pod-side Workflow resolver.
//!
//! Turns `Segment::WorkflowInvoke { slug }` into system-message attachments:
//! dependency Knowledge bodies first, then the Workflow body. Resolution is
//! strict for explicit user invocations: missing workflows, non-user-invocable
//! workflows, and missing Knowledge requirements are returned as errors before
//! the turn is handed to the Worker.

use std::fmt;

use llm_worker::Item;
use memory::WorkspaceLayout;
use memory::schema::split_frontmatter;
use workflow_crate::{Slug, WorkflowRegistry};

#[derive(Debug)]
pub enum WorkflowResolveError {
    InvalidSlug(workflow_crate::WorkflowLintError),
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

struct BuiltinKnowledgeResource {
    slug: &'static str,
    content: &'static str,
}

const BUILTIN_KNOWLEDGE: &[BuiltinKnowledgeResource] = &[BuiltinKnowledgeResource {
    slug: "workflow-resource-boundary",
    content: include_str!("../../../../resources/knowledge/workflow-resource-boundary.md"),
}];

fn builtin_knowledge(slug: &Slug) -> Option<&'static str> {
    BUILTIN_KNOWLEDGE
        .iter()
        .find(|resource| resource.slug == slug.as_str())
        .map(|resource| resource.content)
}

fn read_required_knowledge(
    workflow: &Slug,
    layout: &WorkspaceLayout,
    req: &Slug,
) -> Result<(String, &'static str), WorkflowResolveError> {
    let path = layout.knowledge_dir().join(format!("{req}.md"));
    match std::fs::read_to_string(&path) {
        Ok(raw) => Ok((raw, "workspace")),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            if let Some(raw) = builtin_knowledge(req) {
                Ok((raw.to_string(), "builtin"))
            } else {
                Err(WorkflowResolveError::KnowledgeNotFound {
                    workflow: workflow.to_string(),
                    slug: req.to_string(),
                })
            }
        }
        Err(source) => Err(WorkflowResolveError::KnowledgeRead {
            workflow: workflow.to_string(),
            slug: req.to_string(),
            source,
        }),
    }
}

pub fn resolve_workflow_invocation(
    registry: &WorkflowRegistry,
    layout: &WorkspaceLayout,
    raw_slug: &str,
) -> Result<Vec<Item>, WorkflowResolveError> {
    let slug = Slug::parse(raw_slug.to_string())
        .map_err(|source| WorkflowResolveError::InvalidSlug(source.into()))?;
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
        let (raw, knowledge_source) = read_required_knowledge(&slug, layout, req)?;
        let (_yaml, body) = split_frontmatter(&raw).map_err(|source| {
            WorkflowResolveError::KnowledgeFrontmatter {
                workflow: slug.to_string(),
                slug: req.to_string(),
                source,
            }
        })?;
        out.push(Item::system_message(format!(
            "[Workflow /{} requires Knowledge #{} from {}]\n{}",
            slug,
            req,
            knowledge_source,
            body.trim_end()
        )));
    }
    out.push(Item::system_message(format!(
        "[Workflow /{} from {}]\n{}",
        slug,
        record.source.label(),
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
            &dir.path().join(".yoi/knowledge/policy.md"),
            "---\ncreated_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\nkind: policy\ndescription: p\nmodel_invokation: false\nuser_invocable: true\nlast_sources: []\n---\npolicy body\n",
        );
        write(
            &dir.path().join(".yoi/workflow/run-it.md"),
            "---\ndescription: run\nrequires: [policy]\n---\nworkflow body\n",
        );
        let registry = workflow_crate::load_workflows(&layout).unwrap();
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
        assert!(second.contains("[Workflow /run-it from workspace workflow]"));
        assert!(second.contains("workflow body"));
    }

    #[test]
    fn builtin_workflow_uses_builtin_required_knowledge_when_workspace_missing() {
        let dir = TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        let registry = workflow_crate::load_workflows(&layout).unwrap();
        let items =
            resolve_workflow_invocation(&registry, &layout, "ticket-intake-workflow").unwrap();
        let first = format!("{:?}", items[0]);
        let second = format!("{:?}", items[1]);
        assert!(first.contains("Knowledge #workflow-resource-boundary from builtin"));
        assert!(first.contains("Builtin workflow resources live under"));
        assert!(second.contains("[Workflow /ticket-intake-workflow from builtin workflow]"));
    }

    #[test]
    fn workspace_knowledge_overrides_builtin_required_knowledge() {
        let dir = TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        write(
            &dir.path()
                .join(".yoi/knowledge/workflow-resource-boundary.md"),
            "---\ncreated_at: 2026-01-01T00:00:00Z\nupdated_at: 2026-01-01T00:00:00Z\nkind: policy\ndescription: p\nmodel_invokation: false\nuser_invocable: true\nlast_sources: []\n---\nworkspace override knowledge\n",
        );
        let registry = workflow_crate::load_workflows(&layout).unwrap();
        let items =
            resolve_workflow_invocation(&registry, &layout, "ticket-intake-workflow").unwrap();
        let first = format!("{:?}", items[0]);
        assert!(first.contains("Knowledge #workflow-resource-boundary from workspace"));
        assert!(first.contains("workspace override knowledge"));
    }

    #[test]
    fn user_invocable_false_errors() {
        let (dir, layout, _registry) = setup();
        write(
            &dir.path().join(".yoi/workflow/hidden.md"),
            "---\ndescription: hidden\nuser_invocable: false\n---\nbody\n",
        );
        let registry = workflow_crate::load_workflows(&layout).unwrap();
        let err = resolve_workflow_invocation(&registry, &layout, "hidden").unwrap_err();
        assert!(matches!(err, WorkflowResolveError::NotUserInvocable { .. }));
    }

    #[test]
    fn missing_required_knowledge_errors() {
        let dir = TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(dir.path().to_path_buf());
        write(
            &dir.path().join(".yoi/workflow/bad.md"),
            "---\ndescription: bad\nrequires: [ghost]\n---\nbody\n",
        );
        let registry = workflow_crate::load_workflows(&layout).unwrap();
        let err = resolve_workflow_invocation(&registry, &layout, "bad").unwrap_err();
        assert!(matches!(
            err,
            WorkflowResolveError::KnowledgeNotFound { .. }
        ));
    }
}
