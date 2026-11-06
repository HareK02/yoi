//! Real domain details share the DTO/schema source without sharing a specialized component.
//! Run in the normal server-api integration suite; no transport or storage fixtures are needed.
#![allow(dead_code)]

use api_macros::openapi::{OpenApiBuilder, OpenApiInfo, OpenApiSchema};
use error_contract::{codec::FailureEnvelope, problem::ProblemDetails};
use schemars::JsonSchema;
use serde_json::Value;
use server_api::{
    configuration_failure::ConfigurationFailureDetails,
    repository_access_failure::RepositoryAccessDetails,
};
use workdir::failure::WorkdirFailureDetails;

#[derive(JsonSchema)]
struct RepositoryDocument {
    failure: FailureEnvelope<RepositoryAccessDetails>,
    problem: ProblemDetails<RepositoryAccessDetails>,
}
impl OpenApiSchema for RepositoryDocument {}
#[derive(JsonSchema)]
struct ConfigurationDocument {
    failure: FailureEnvelope<ConfigurationFailureDetails>,
    problem: ProblemDetails<ConfigurationFailureDetails>,
}
impl OpenApiSchema for ConfigurationDocument {}
#[derive(JsonSchema)]
struct WorkdirDocument {
    failure: FailureEnvelope<WorkdirFailureDetails>,
    problem: ProblemDetails<WorkdirFailureDetails>,
}
impl OpenApiSchema for WorkdirDocument {}

fn builder() -> OpenApiBuilder {
    OpenApiBuilder::new(OpenApiInfo {
        title: "Mixed domain schema identity regression",
        version: "1",
        source_digest: "test:domains",
    })
    .unwrap()
}
fn component_name(reference: &Value) -> &str {
    reference["$ref"]
        .as_str()
        .unwrap()
        .strip_prefix("#/components/schemas/")
        .unwrap()
}

#[test]
fn mixed_domains_keep_distinct_details_references_and_same_domain_registration_is_idempotent() {
    let mut builder = builder();
    let repository = builder.schema_ref::<RepositoryDocument>().unwrap();
    assert_eq!(
        repository,
        builder.schema_ref::<RepositoryDocument>().unwrap()
    );
    let configuration = builder.schema_ref::<ConfigurationDocument>().unwrap();
    let workdir = builder.schema_ref::<WorkdirDocument>().unwrap();
    let document: Value =
        serde_json::from_str(&builder.finish().unwrap().to_json().unwrap()).unwrap();
    let schemas = &document["components"]["schemas"];
    let mut envelope_refs = Vec::new();
    let mut problem_refs = Vec::new();
    for (reference, details_name) in [
        (repository, "RepositoryAccessDetails"),
        (configuration, "ConfigurationFailureDetails"),
        (workdir, "WorkdirFailureDetails"),
    ] {
        let properties = &schemas[component_name(&reference)]["properties"];
        let envelope_ref = &properties["failure"];
        let problem_ref = &properties["problem"];
        let envelope = &schemas[component_name(envelope_ref)];
        assert_eq!(
            component_name(&envelope["properties"]["details"]),
            details_name
        );
        let problem = &schemas[component_name(problem_ref)];
        assert_eq!(&problem["properties"]["yoi_failure"], envelope_ref);
        assert!(
            !envelope_refs.contains(envelope_ref),
            "different details shared a component"
        );
        assert!(
            !problem_refs.contains(problem_ref),
            "different problems shared a component"
        );
        envelope_refs.push(envelope_ref.clone());
        problem_refs.push(problem_ref.clone());
    }
}

#[derive(JsonSchema)]
#[schemars(rename = "WorkdirFailureDetails")]
struct ConflictingDetails {
    unrelated: bool,
}
impl OpenApiSchema for ConflictingDetails {}

#[test]
fn a_real_same_name_different_schema_collision_is_still_rejected() {
    let mut builder = builder();
    builder.schema_ref::<WorkdirDocument>().unwrap();
    let error = builder.schema_ref::<ConflictingDetails>().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("component name collision for `WorkdirFailureDetails`")
    );
}
