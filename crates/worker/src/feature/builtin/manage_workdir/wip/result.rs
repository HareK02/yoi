//! Native result declarations; identity data never acquires Entry semantics by its spelling.
use super::Domain;
use wip_protocol::{Documentation, EnumCase, FieldDeclaration, TypeExpr, UnionCase};

fn field(name: &str, required: bool, ty: TypeExpr) -> FieldDeclaration {
    FieldDeclaration {
        name: name.into(),
        required,
        documentation: None,
        r#type: ty,
    }
}
fn record(fields: Vec<FieldDeclaration>) -> TypeExpr {
    TypeExpr::Record { fields }
}

pub(super) fn item(domain: Domain) -> TypeExpr {
    let mut fields = vec![field("path", true, TypeExpr::Entry)];
    let data: &[&str] = match domain {
        Domain::Repository => &[
            "repository_key",
            "kind",
            "provider",
            "display_name",
            "description",
            "default_selector",
        ],
        Domain::Workdir => &[
            "working_directory_id",
            "display_name",
            "creation_selector",
            "creation_ref",
            "creation_tree",
            "current_selector",
            "current_ref",
            "current_tree",
            "status",
            "cleanliness",
            "occupied_by",
        ],
        Domain::Attachment => &[
            "connection_id",
            "alias",
            "working_directory_id",
            "capabilities",
            "name",
            "purpose",
            "access",
        ],
    };
    let identities: &[&str] = match domain {
        Domain::Repository => &["repository_key"],
        Domain::Workdir => &["working_directory_id"],
        Domain::Attachment => &["connection_id", "alias", "working_directory_id"],
    };
    for name in data {
        // Nullable/structured Backend metadata remains ordinary JSON, never navigable references.
        fields.push(if identities.contains(name) {
            field(name, true, TypeExpr::String)
        } else {
            field(name, false, TypeExpr::Json)
        });
    }
    if matches!(domain, Domain::Workdir) {
        fields.push(field(
            "source",
            false,
            record(vec![
                field("kind", true, TypeExpr::String),
                field("repository_key", false, TypeExpr::String),
                field("repository_path", false, TypeExpr::Entry),
                field("access", false, TypeExpr::String),
                // Source mapping only: no attached connection, therefore not an Entry.
                field("content_path", false, TypeExpr::String),
                field("purpose", false, TypeExpr::String),
            ]),
        ));
    }
    if matches!(domain, Domain::Attachment) {
        for name in ["workdir_path", "checkout_path", "content_path"] {
            fields.push(field(name, false, TypeExpr::Entry));
        }
        fields.push(field(
            "content_state",
            true,
            TypeExpr::Enum {
                cases: [
                    ("available", "Current entrance coordinate only; content and access are not acquired"),
                    ("unpublished", "Native content subtree is not registered; a self-attach root alone is insufficient"),
                    ("disconnected", "No matching current content connection/session"),
                    ("unsupported", "Attached session does not support content Read"),
                    ("unavailable", "Exact connection could not be confirmed; rediscover, do not repeat mutation"),
                ]
                .into_iter()
                .map(|(name, summary)| EnumCase {
                    name: name.into(),
                    documentation: Some(Documentation { summary:summary.into(), details:None }),
                })
                .collect(),
            },
        ));
    }
    record(fields)
}

pub(super) fn page(domain: Domain) -> TypeExpr {
    record(vec![
        field(
            "items",
            true,
            TypeExpr::List {
                items: Box::new(item(domain)),
            },
        ),
        field("empty", true, TypeExpr::Boolean),
        field("has_more", true, TypeExpr::Boolean),
        field("next_cursor", true, TypeExpr::Json),
        field("limit", true, TypeExpr::Integer),
    ])
}
pub(super) fn created(domain: Domain) -> TypeExpr {
    record(vec![
        field("item", true, item(domain)),
        field("runtime_id", true, TypeExpr::String),
    ])
}
pub(super) fn attached() -> TypeExpr {
    record(vec![
        field("workspace_id", true, TypeExpr::String),
        field("alias", true, TypeExpr::String),
        field("working_directory_id", true, TypeExpr::String),
        field("capabilities", true, TypeExpr::Json),
        field("environment", true, TypeExpr::Json),
        field("attached", true, TypeExpr::Boolean),
        field("workdir_path", true, TypeExpr::Entry),
        field("attachments_path", true, TypeExpr::Entry),
        field("attachment", true, TypeExpr::Union { cases: vec![
            UnionCase { name:"available".into(), documentation:None, payload:Some(item(Domain::Attachment)) },
            UnionCase { name:"unavailable".into(), documentation:Some(Documentation {
                summary:"Attach succeeded; reference lookup unavailable. Rediscover attachments; do not repeat attach.".into(), details:None
            }), payload:None },
        ] }),
    ])
}
