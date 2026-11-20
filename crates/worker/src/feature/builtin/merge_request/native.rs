//! Native v2 input contracts and their domain-command adapter.
//!
//! v1 exposed flattened Tool-shaped fields and `check_readiness`. v2 deliberately has no aliases:
//! consumers must Inspect the current Object and then its Interface again, use Ticket Entries,
//! group repository/page/integration records, and invoke `readiness`. Tool mode is unchanged.
//!
//! Examples below show Invoke's JSON binding, not Tool JSON. Read the allocated Ticket collection
//! root from the Entry parameter's Inspect documentation (it need not be `/tickets`). This input
//! coordinate remains available when Ticket Objects are unpublished; it is not an observation or grant.
//!
//! ```text
//! collection.open({ticket: "/tickets/T-797",
//!   repository: {key: "main", source: "work/T-797", target: "develop"}})
//! item.read({page: {after: {$case: "value", value: 7},
//!   limit: {$case: "value", value: 25}}})
//! item.readiness({})
//! item.review({decision: "request_changes", findings: [
//!   {severity: "major", path: {$case: "value", value: "src/main.rs"},
//!    line: {$case: "value", value: 12}, code: {$case: "null"}, body: "Explain the defect"}]})
//! item.complete({operation_id: "integration-1", integration: {
//!   approval_event_id: "approved-1", target: {before: "target-1", after: "result-1"},
//!   strategy: "fast_forward", resolution: "clean"}})
//! ```
//!
//! Optional fields may be omitted; only Nullable shapes accept explicit null, represented as
//! `{$case: "null"}` or `{$case: "value", value: T}` because WIP has no nullable primitive.
//! Plain JSON null is not accepted. Omitted summary/body/findings use domain defaults. Backend
//! remains authoritative for candidate freshness, exact approval, target observations, and business
//! eligibility. Page bounds and finding line integer ranges are checked before authorization and
//! dispatch, separately from native type validation. Complete records integration; it never applies
//! Git changes. Unknown dispatched outcomes are not retried.

use super::*;
pub(super) use crate::feature::builtin::native_contract::Contract;
use crate::feature::builtin::native_contract::{Destination, Input, ReferenceKind, Shape};

/// Owned, per-mount roots keep handlers independent of registry lifetime. Empty means that
/// collection was not allocated; reference conversion then fails closed, with no default route.
#[derive(Clone, Default)]
pub(super) struct ReferenceRoots {
    ticket: String,
    merge_request: String,
    objective: String,
}

pub(super) fn reference_roots(registry: &WipMountRegistry) -> ReferenceRoots {
    ReferenceRoots {
        ticket: registry.namespace_root("ticket").unwrap_or_default().into(),
        objective: registry
            .namespace_root("objective")
            .unwrap_or_default()
            .into(),
        merge_request: registry
            .namespace_root("merge-request")
            .unwrap_or_default()
            .into(),
    }
}

fn field(name: &'static str, required: bool, ty: Shape) -> Input {
    Input {
        name,
        required,
        ty,
        destination: Destination::Same,
    }
}

fn renamed(name: &'static str, required: bool, ty: Shape, destination: &'static str) -> Input {
    Input {
        name,
        required,
        ty,
        destination: Destination::Rename(destination),
    }
}

fn flattened(name: &'static str, required: bool, fields: Vec<Input>) -> Input {
    Input {
        name,
        required,
        ty: Shape::Record(fields),
        destination: Destination::Flatten,
    }
}

fn nullable(ty: Shape) -> Shape {
    Shape::Nullable(Box::new(ty))
}

pub(super) fn contract(kind: Kind) -> Contract {
    let (name, summary, parameters) = match kind {
        Kind::Open => (
            "open",
            "Open or reuse a Ticket-linked repository candidate. ticket is an Entry in the current Ticket collection; repository.source/target are selectors, not asserted commit evidence. Backend validates linkage and authority.",
            vec![
                field("ticket", true, Shape::Reference(ReferenceKind::Ticket)),
                flattened(
                    "repository",
                    true,
                    vec![
                        renamed("key", true, Shape::String, "repository_key"),
                        renamed("source", true, Shape::String, "selector_from"),
                        renamed("target", true, Shape::String, "selector_to"),
                    ],
                ),
                field("summary", false, Shape::String),
            ],
        ),
        Kind::Show => (
            "read",
            "Read this route-bound MR. Default summary omits bodies/history; view=detail returns a bounded thread page. Supplying page fields without view selects detail. page.after is exclusive (0..9007199254740991); page.limit is 1..200. Summary cannot be combined with page fields. Optional page fields use null/value unions when explicitly present.",
            vec![
                field("view", false, Shape::Enum(vec!["summary", "detail"])),
                flattened(
                    "page",
                    false,
                    vec![
                        field("after", false, nullable(Shape::Integer)),
                        field("limit", false, nullable(Shape::Integer)),
                    ],
                ),
            ],
        ),
        Kind::WorkerQuery => (
            "worker_query",
            "Read this route-bound MR's single Worker assignment. assignment=null means never assigned; worker=null in a retained assignment is a release with a retained operation_id. This observation grants no authority and does not report Worker liveness or Ticket assignment.",
            vec![],
        ),
        Kind::WorkerSet => (
            "worker_set",
            "Set, replace, or release this MR's Worker using the queried operation_id as expected_operation_id. Both expected_operation_id and worker are required null/value unions; null expectation means never assigned, null worker releases ownership. Preserve operation_id and full input for exact retry; reconcile conflict or unknown outcome without automatic retry or a new key. Ownership changes no Ticket assignment, authority, lifecycle, phase, approval, or publication.",
            vec![
                field("operation_id", true, Shape::String),
                field("expected_operation_id", true, nullable(Shape::String)),
                field(
                    "worker",
                    true,
                    nullable(Shape::Record(vec![
                        field("runtime_id", true, Shape::String),
                        field("worker_id", true, Shape::String),
                    ])),
                ),
            ],
        ),
        Kind::Comment => (
            "comment",
            "Append an owner-fenced Markdown thread comment. Use the observed assignment operation_id and retain the same full input for exact replay. A comment is not approval, authority or publication evidence.",
            vec![
                field("operation_id", true, Shape::String),
                field("expected_owner_operation_id", true, Shape::String),
                field("body", true, Shape::String),
            ],
        ),
        Kind::Publications => (
            "publications",
            "Read saved publication operations and outcomes. This does not send, retry, or conclude work.",
            vec![],
        ),
        Kind::Publish => (
            "publish",
            "Publish the exact approved source through the current MR Worker Workdir. Preserve operation_id and approval_event_id for exact replay; a saved operation only reconciles, never redispatches. Unknown is not failure. No Subject stamp or Ticket conclusion.",
            vec![
                field("operation_id", true, Shape::String),
                field("approval_event_id", true, Shape::String),
            ],
        ),
        Kind::Readiness => (
            "readiness",
            "Check current review and exact-source integration eligibility for this MR; no integration or Ticket conclusion is performed.",
            vec![],
        ),
        Kind::Review => (
            "review",
            "Record an injected Reviewer's decision on this exact MR. decision is required. Omit body/findings for empty defaults; finding severity/body are required, optional code/path/line use null/value unions, and a present line is 0..4294967295. Backend validates candidate freshness; approve triggers automatic publication when current permissions and target conditions permit. Model inputs never carry a capability.",
            vec![
                field(
                    "decision",
                    true,
                    Shape::Enum(vec!["approve", "request_changes"]),
                ),
                field("body", false, Shape::String),
                field(
                    "findings",
                    false,
                    Shape::List(Box::new(Shape::Record(vec![
                        field(
                            "severity",
                            true,
                            Shape::Enum(vec!["blocker", "major", "minor", "note"]),
                        ),
                        field("code", false, nullable(Shape::String)),
                        field("path", false, nullable(Shape::String)),
                        field("line", false, nullable(Shape::Integer)),
                        field("body", true, Shape::String),
                    ]))),
                ),
            ],
        ),
        Kind::Complete => (
            "complete",
            "Reconcile completion from the saved provider-confirmed publication operation. All integration fields must match that operation. operation_id is the stable replay key; Backend rechecks current approval and requirements without a Subject stamp. No new Git operation, automatic retry, or Ticket closure occurs.",
            vec![
                field("operation_id", true, Shape::String),
                flattened(
                    "integration",
                    true,
                    vec![
                        field("approval_event_id", true, Shape::String),
                        flattened(
                            "target",
                            true,
                            vec![
                                renamed("before", true, Shape::String, "target_ref_before"),
                                renamed("after", true, Shape::String, "target_ref_after"),
                            ],
                        ),
                        field("strategy", true, Shape::Enum(vec!["fast_forward", "merge"])),
                        field(
                            "resolution",
                            true,
                            Shape::Enum(vec!["none", "clean", "conflicts_resolved"]),
                        ),
                    ],
                ),
            ],
        ),
    };
    Contract {
        name,
        summary,
        parameters,
    }
}

impl ReferenceRoots {
    fn contract_roots(&self) -> crate::feature::builtin::native_contract::ReferenceRoots<'_> {
        crate::feature::builtin::native_contract::ReferenceRoots {
            ticket: &self.ticket,
            merge_request: &self.merge_request,
            objective: &self.objective,
        }
    }
}

pub(super) fn declaration(contract: &Contract, roots: &ReferenceRoots) -> OperationDeclaration {
    let mut declaration = contract.declaration(&roots.contract_roots());
    declaration.returns = result::declaration();
    declaration
}

pub(super) fn convert(
    contract: &Contract,
    arguments: &BTreeMap<String, WipValue>,
    roots: &ReferenceRoots,
) -> Result<Value, WipOperationError> {
    contract.convert(arguments, &roots.contract_roots())
}

pub(super) fn command(kind: Kind, mut input: Value) -> Result<command::Command, WipOperationError> {
    if kind == Kind::Show
        && input
            .get("after")
            .and_then(Value::as_u64)
            .is_some_and(|after| after > 9_007_199_254_740_991)
    {
        return Err(merge_request_protocol_failure(
            ProtocolErrorCode::InvalidArguments,
            "page.after exceeds the lossless integer range",
        ));
    }
    input
        .as_object_mut()
        .expect("native contract produces a record")
        .insert("command".into(), Value::String(contract(kind).name.into()));
    serde_json::from_value(input).map_err(|error| {
        merge_request_protocol_failure(
            ProtocolErrorCode::InvalidArguments,
            format!("native command: {error}"),
        )
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod domain_tests;

#[cfg(test)]
mod ownership_tests;

#[cfg(test)]
mod publication_tests;
