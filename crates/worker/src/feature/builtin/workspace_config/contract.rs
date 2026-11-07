//! The native batch shape owns both its declaration and conversion to the Backend JSON command.
//!
//! WIP unions use `$case`/`value`, not the Backend's `kind`-tagged JSON. Reusing the native
//! contract converter keeps these representations explicit without accepting a legacy JSON fallback.
//! Semantic validation and atomic persistence remain exclusively at the canonical Backend boundary.
use std::collections::BTreeMap;

use serde_json::Value as Json;
use wip_protocol::{Documentation, FieldDeclaration, OperationDeclaration, TypeExpr, Value};

use super::backend::{MAX_NODES, MAX_PATH_BYTES, MAX_TEXT_BYTES};
use crate::feature::builtin::native_contract::{
    Case, Contract, Destination, Input, ReferenceRoots, Shape,
};
use crate::wip::WipOperationError;

pub(super) const ACTIVATION: &str =
    "Saved to canonical Workspace configuration; existing Worker manifests are not rewritten";

// This command has no Entry fields and therefore uses no domain reference coordinates.
const ROOTS: ReferenceRoots<'static> = ReferenceRoots {
    ticket: "",
    merge_request: "",
    objective: "",
};

fn field(name: &'static str, required: bool, ty: Shape) -> Input {
    Input {
        name,
        required,
        ty,
        destination: Destination::Same,
    }
}

fn input() -> Contract {
    Contract {
        name: "apply_changes",
        summary: "Atomically save a configuration batch; validators and digests are managed by the Client/Host, never supplied by the model",
        parameters: vec![field(
            "changes",
            true,
            Shape::List(Box::new(Shape::Union {
                tag: "kind",
                cases: vec![
                    Case {
                        name: "create",
                        payload: Some(Shape::Record(vec![
                            field("path", true, Shape::String),
                            field("content", true, Shape::String),
                            field("content_type", false, Shape::Enum(vec!["decodal", "text"])),
                        ])),
                    },
                    Case {
                        name: "update",
                        payload: Some(Shape::Record(vec![
                            field("path", true, Shape::String),
                            field("content", true, Shape::String),
                        ])),
                    },
                    Case {
                        name: "delete",
                        payload: Some(Shape::Record(vec![field("path", true, Shape::String)])),
                    },
                    Case {
                        name: "rename",
                        payload: Some(Shape::Record(vec![
                            field("from", true, Shape::String),
                            field("to", true, Shape::String),
                        ])),
                    },
                ],
            })),
        )],
    }
}

fn documentation(summary: impl Into<String>) -> Option<Documentation> {
    Some(Documentation {
        summary: summary.into(),
        details: None,
    })
}

pub(super) fn declaration() -> OperationDeclaration {
    let mut operation = input().declaration(&ROOTS);
    operation.documentation = documentation(format!(
        "Atomically validate and save 1..={MAX_NODES} base-relative changes; aggregate content <= {MAX_TEXT_BYTES} UTF-8 bytes. \
        path/from/to are nonempty canonical paths relative to the configuration root (not WIP or OS paths), <= {MAX_PATH_BYTES} UTF-8 bytes: \
        no leading slash, backslash, control characters, URL, empty, . or .. components; $builtin namespace is read-only. \
        Every touched path, including both rename endpoints, may occur only once per batch. Create requires absence; update/delete/rename.from require an existing file; rename.to must be absent. \
        Update retains content_type; rename retains bytes/type and does not rewrite imports. Create content_type defaults to decodal when omitted (not null). \
        The complete candidate must pass Backend import/evaluation/schema validation at the existing entrypoints; update importers in the same batch when renaming/deleting referenced sources. \
        The tree is bounded to {entry_count} entries and {tree_bytes} total UTF-8 bytes, including Backend source/import limits. \
        Validators, expected_digest and entrypoints are automatic, not arguments. Whole-tree CAS rejects stale observations; Inspect again before deciding a new operation. \
        Success returns digest and activation, not per-change results. Rejection is a protocol error with no commit; after dispatched failure OutcomeUnknown is distinct and must not be automatically retried. \
        Backend formats Decodal sources before saving; the returned digest describes the canonical saved tree. \
        Saving is not hot application to running Workers. \
        Invoke JSON encodes each WIP union as {{\"$case\":\"create|update|delete|rename\",\"value\":{{...fields}}}}; \
        the adapter maps it to Backend JSON {{\"kind\":\"case\",...fields}}, adding observed preconditions. Raw kind-tagged input is not accepted. \
        Examples (paths must satisfy the above existence constraints): \
        create {{\"$case\":\"create\",\"value\":{{\"path\":\"notes/new.txt\",\"content\":\"new\",\"content_type\":\"text\"}}}}; \
        update {{\"$case\":\"update\",\"value\":{{\"path\":\"notes/old.txt\",\"content\":\"updated\"}}}}; \
        delete {{\"$case\":\"delete\",\"value\":{{\"path\":\"notes/obsolete.txt\"}}}}; \
        rename {{\"$case\":\"rename\",\"value\":{{\"from\":\"notes/source.txt\",\"to\":\"notes/moved.txt\"}}}}. \
        A composite Invoke arguments record is {{\"changes\":[{{\"$case\":\"create\",\"value\":{{\"path\":\"notes/new.txt\",\"content\":\"new\",\"content_type\":\"text\"}}}},{{\"$case\":\"update\",\"value\":{{\"path\":\"notes/old.txt\",\"content\":\"updated\"}}}},{{\"$case\":\"delete\",\"value\":{{\"path\":\"notes/obsolete.txt\"}}}},{{\"$case\":\"rename\",\"value\":{{\"from\":\"notes/source.txt\",\"to\":\"notes/moved.txt\"}}}}]}}; all changes commit together or none do.",
        entry_count = config_source::MAX_ENTRY_COUNT,
        tree_bytes = config_source::MAX_TOTAL_BYTES,
    ));
    operation.returns.r#type = TypeExpr::Record {
        fields: vec![
            FieldDeclaration {
                name: "digest".into(),
                required: true,
                r#type: TypeExpr::String,
                documentation: documentation(
                    "Digest of the saved whole configuration tree, not a per-file digest or proof of hot activation",
                ),
            },
            FieldDeclaration {
                name: "activation".into(),
                required: true,
                r#type: TypeExpr::String,
                documentation: documentation(ACTIVATION),
            },
        ],
    };
    operation.returns.documentation = documentation(
        "Successful canonical save only. Failures are WIP protocol errors or OutcomeUnknown, never a success record with partial changes; Client validators are returned separately as protocol metadata",
    );
    operation
}

pub(super) fn convert(arguments: &BTreeMap<String, Value>) -> Result<Json, WipOperationError> {
    input().convert(arguments, &ROOTS)
}
