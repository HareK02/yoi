use super::*;
use serde_json::json;

fn digest(hex: char) -> String {
    format!("sha256:{}", hex.to_string().repeat(64))
}

fn artifact() -> ArtifactIdentityProjection {
    ArtifactIdentityProjection {
        package_id: "package".into(),
        version: "1.0.0".into(),
        package_digest: digest('a'),
        artifact_digest: digest('b'),
        artifact_size: 42,
        schema_version: 2,
        world: plugin::PLUGIN_COMPONENT_HTTPS_LOGGED_TOOL_WORLD.into(),
        required_imports: vec!["logging".into(), "https".into(), "logging".into()],
        tools: vec!["other".into(), "echo".into()],
        connections: vec!["api".into()],
        requested_grants: vec![
            PluginGrant::Https { slot: "api".into() },
            PluginGrant::Logging,
        ],
        config_schema: PluginSchemaIdentity {
            identity: "config".into(),
            version: "1".into(),
            digest: digest('c'),
            namespace: "example".into(),
        },
        revoked: false,
    }
}

fn projection() -> PluginPinProjection {
    PluginPinProjection {
        config: serde_json::from_str(r#"{"z":{"b":2,"a":1},"a":[{"z":0,"a":false}]}"#).unwrap(),
        surfaces: vec![PluginSurface::Tool, PluginSurface::Tool],
        tools: vec!["other".into(), "echo".into(), "other".into()],
        grants: vec![
            PluginGrant::Https { slot: "api".into() },
            PluginGrant::Logging,
        ],
        bindings: vec![BindingPolicyReference {
            slot: "api".into(),
            binding_id: "binding".into(),
            policy_digest: digest('f'),
        }],
    }
}

fn pin(
    artifact: &impl ArtifactDescriptor,
    projection: PluginPinProjection,
) -> Result<WorkerPluginPin> {
    WorkerPluginPin::from_artifact(
        "installation".into(),
        "operation".into(),
        artifact,
        projection,
    )
}

fn input() -> WorkerPluginPlanInput {
    WorkerPluginPlanInput {
        workspace_id: "workspace".into(),
        config: ConfigProvenance {
            content_digest: digest('d'),
            toolchain_fingerprint: "toolchain".into(),
        },
        profile: ProfileProvenance {
            identity: "profile:coder".into(),
            projection_digest: digest('e'),
        },
        execution: HostExecutionIdentity {
            runtime_id: "runtime".into(),
            worker_id: "worker".into(),
            principal_id: "principal".into(),
            delegation_id: Some("delegation".into()),
        },
        plugins: vec![pin(&artifact(), projection()).unwrap()],
    }
}

#[test]
fn canonical_wire_and_fingerprint_preserve_persisted_format() {
    let plan = WorkerPluginExecutionPlan::canonicalize(input()).unwrap();
    let expected = include_str!("fixtures/canonical-plan.json").trim_end();
    assert_eq!(plan.canonical_json().unwrap(), expected);
    assert_eq!(
        plan.fingerprint().unwrap(),
        "sha256:00ee15d79098f4afa137da5d4b0b9b78acde48a6157fdda0f3327d8c122bd889"
    );
    let restored: WorkerPluginExecutionPlan = serde_json::from_str(expected).unwrap();
    restored.validate_canonical().unwrap();
    assert_eq!(restored, plan);
    assert!(restored.plugins()[0].matches_artifact(&artifact()).unwrap());
}

#[test]
fn artifact_identity_changes_are_not_hidden_by_projection_adapter() {
    let original = pin(&artifact(), projection()).unwrap();
    let mut changed = artifact();
    changed.artifact_size += 1;
    assert!(!original.matches_artifact(&changed).unwrap());
    changed = artifact();
    changed.config_schema.digest = digest('d');
    assert!(!original.matches_artifact(&changed).unwrap());
    changed = artifact();
    changed.revoked = true;
    assert_eq!(
        original.matches_artifact(&changed),
        Err(PlanError::Conflict)
    );
}

#[test]
fn unsupported_or_malformed_artifact_identity_is_invalid() {
    for change in [
        |a: &mut ArtifactIdentityProjection| a.package_digest = "sha256:ABC".into(),
        |a: &mut ArtifactIdentityProjection| a.artifact_digest = "digest".into(),
        |a: &mut ArtifactIdentityProjection| a.artifact_size = 0,
        |a: &mut ArtifactIdentityProjection| a.schema_version = 1,
        |a: &mut ArtifactIdentityProjection| a.world = "unsupported".into(),
        |a: &mut ArtifactIdentityProjection| a.package_id.clear(),
        |a: &mut ArtifactIdentityProjection| a.version = "bad\nversion".into(),
        |a: &mut ArtifactIdentityProjection| a.config_schema.identity.clear(),
        |a: &mut ArtifactIdentityProjection| a.config_schema.version.clear(),
        |a: &mut ArtifactIdentityProjection| a.config_schema.digest.clear(),
        |a: &mut ArtifactIdentityProjection| a.config_schema.namespace.clear(),
    ] {
        let mut value = artifact();
        change(&mut value);
        assert_eq!(pin(&value, projection()), Err(PlanError::Invalid));
    }
}

#[test]
fn selected_tools_grants_and_bindings_cannot_expand_artifact_metadata() {
    for change in [
        |p: &mut PluginPinProjection| p.config = json!([]),
        |p: &mut PluginPinProjection| p.surfaces.clear(),
        |p: &mut PluginPinProjection| p.tools.push("absent".into()),
        |p: &mut PluginPinProjection| p.bindings.push(p.bindings[0].clone()),
        |p: &mut PluginPinProjection| p.bindings[0].slot = "absent".into(),
        |p: &mut PluginPinProjection| p.bindings[0].binding_id.clear(),
        |p: &mut PluginPinProjection| p.bindings[0].policy_digest.clear(),
        |p: &mut PluginPinProjection| p.bindings.clear(),
        |p: &mut PluginPinProjection| p.grants.retain(|g| *g == PluginGrant::Logging),
    ] {
        let mut value = projection();
        change(&mut value);
        assert_eq!(pin(&artifact(), value), Err(PlanError::Invalid));
    }
    for change in [
        |a: &mut ArtifactIdentityProjection| a.tools.clear(),
        |a: &mut ArtifactIdentityProjection| a.connections.clear(),
        |a: &mut ArtifactIdentityProjection| a.requested_grants.clear(),
    ] {
        let mut value = artifact();
        change(&mut value);
        assert_eq!(pin(&value, projection()), Err(PlanError::Invalid));
    }
}

#[test]
fn malformed_provenance_and_duplicate_installations_are_invalid() {
    for change in [
        |i: &mut WorkerPluginPlanInput| i.workspace_id.clear(),
        |i: &mut WorkerPluginPlanInput| i.config.content_digest.clear(),
        |i: &mut WorkerPluginPlanInput| i.config.toolchain_fingerprint.clear(),
        |i: &mut WorkerPluginPlanInput| i.profile.identity.clear(),
        |i: &mut WorkerPluginPlanInput| i.profile.projection_digest.clear(),
        |i: &mut WorkerPluginPlanInput| i.execution.runtime_id.clear(),
        |i: &mut WorkerPluginPlanInput| i.execution.worker_id.clear(),
        |i: &mut WorkerPluginPlanInput| i.execution.principal_id.clear(),
        |i: &mut WorkerPluginPlanInput| i.execution.delegation_id = Some(String::new()),
        |i: &mut WorkerPluginPlanInput| i.plugins.push(i.plugins[0].clone()),
        |i: &mut WorkerPluginPlanInput| i.plugins = vec![i.plugins[0].clone(); 129],
    ] {
        let mut value = input();
        change(&mut value);
        assert_eq!(
            WorkerPluginExecutionPlan::canonicalize(value),
            Err(PlanError::Invalid)
        );
    }
}

#[test]
fn deserialized_unknown_fields_versions_and_noncanonical_order_are_rejected() {
    let plan = WorkerPluginExecutionPlan::canonicalize(input()).unwrap();
    let mut wire = serde_json::to_value(&plan).unwrap();
    wire["input"]["execution"]["token"] = json!("credential");
    assert!(serde_json::from_value::<WorkerPluginExecutionPlan>(wire).is_err());
    let mut wire = serde_json::to_value(&plan).unwrap();
    wire["format_version"] = json!(2);
    let unsupported: WorkerPluginExecutionPlan = serde_json::from_value(wire).unwrap();
    assert_eq!(unsupported.validate_canonical(), Err(PlanError::Invalid));
    let mut unsorted = plan.clone();
    let mut second = unsorted.input.plugins[0].clone();
    second.installation_id = "aaa".into();
    unsorted.input.plugins.push(second);
    assert_eq!(unsorted.validate_canonical(), Err(PlanError::Invalid));
}

#[test]
fn ticket_grants_require_exact_world_request_and_schema_namespace() {
    let mut a = artifact();
    let write = PluginGrant::TicketDataWrite {
        collection: "notes".into(),
        schema_id: "note".into(),
        schema_version: "1".into(),
    };
    let read = PluginGrant::TicketDataRead {
        collection: "notes".into(),
        schema_id: "note".into(),
        schema_version: "1".into(),
    };
    a.requested_grants
        .extend([PluginGrant::TicketRead, write.clone()]);
    let mut selected = projection();
    selected.grants.push(write.clone());
    // Even a fabricated metadata request cannot add Ticket operations to v2.0.
    assert_eq!(pin(&a, selected.clone()), Err(PlanError::Invalid));
    a.world = plugin::PLUGIN_COMPONENT_TICKET_TOOL_WORLD.into();
    let exact = pin(&a, selected.clone()).unwrap();
    assert!(exact.projection().grants.contains(&write));
    assert!(!exact.projection().grants.contains(&read)); // write is not read
    selected.grants.push(read);
    assert_eq!(pin(&a, selected), Err(PlanError::Invalid));
    let mut selected = projection();
    selected.grants.push(PluginGrant::TicketDataWrite {
        collection: "notes".into(),
        schema_id: "note".into(),
        schema_version: "2".into(),
    });
    assert_eq!(pin(&a, selected), Err(PlanError::Invalid));
    a.requested_grants.retain(|g| g != &write);
    assert_eq!(exact.matches_artifact(&a), Err(PlanError::Invalid));
}
