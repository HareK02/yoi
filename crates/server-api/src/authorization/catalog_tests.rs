//! Management validation is exercised from JSON; stored legacy policy remains readable.
use super::*;
use serde_json::json;

fn permission(operation: &str, resource: serde_json::Value) -> ScopedPermission {
    serde_json::from_value(json!({"operation":operation,"resource":resource})).unwrap()
}
fn update(
    execution: Vec<ScopedPermission>,
    delegation: Vec<ScopedPermission>,
) -> SubjectPermissionsUpdate {
    SubjectPermissionsUpdate {
        operation_id: "save".into(),
        expected_change_id: Some("observed".into()),
        execution,
        delegation,
    }
}

#[test]
fn managed_additions_reject_unknown_mismatched_and_unavailable_entries() {
    let workspace = json!({"kind":"workspace","workspace_id":"w"});
    for grant in [
        permission("ticket.future", workspace.clone()),
        permission("ticket.read", workspace.clone()),
        permission(
            "ticket.create",
            json!({"kind":"ticket","workspace_id":"w","ticket_id":"T-1"}),
        ),
        permission("worker.messaging", workspace.clone()),
        permission(
            "worker.stop",
            json!({"kind":"worker","runtime_id":"r","worker_id":"worker"}),
        ),
        permission("ticket", json!({"kind":"feature"})),
        permission(
            "bash.execute",
            json!({"kind":"workdir","working_directory_id":"wd","path":"src","recursive":true}),
        ),
    ] {
        assert!(
            validate_managed_subject_permissions(None, &update(vec![grant.clone()], vec![]))
                .is_err(),
            "{grant:?}"
        );
        assert!(
            validate_managed_subject_permissions(None, &update(vec![], vec![grant.clone()]))
                .is_err(),
            "{grant:?}"
        );
    }
    assert!(validate_managed_subject_permissions(None, &update(vec![permission("ticket.create", workspace)], vec![permission("filesystem.write", json!({"kind":"workdir","working_directory_id":"wd","path":"src","recursive":true}))])).is_ok());
}

#[test]
fn unsupported_legacy_entries_are_exact_lane_preserved_not_converted_or_extended() {
    let legacy: SubjectPermissions = serde_json::from_value(json!({
        "change_id":"observed",
        "execution":[
            {"operation":"ticket","resource":{"kind":"feature"}},
            {"operation":"worker.notify","resource":{"kind":"worker","runtime_id":"r","worker_id":"one"}},
            {"operation":"old.unknown","resource":{"kind":"workspace","workspace_id":"w"}}
        ],
        "delegation":[{"operation":"worker.result","resource":{"kind":"worker","runtime_id":"r","worker_id":"origin"}}]
    })).unwrap();
    let mut next = update(legacy.execution.clone(), legacy.delegation.clone());
    next.execution.push(permission(
        "ticket.create",
        json!({"kind":"workspace","workspace_id":"w"}),
    ));
    assert!(validate_managed_subject_permissions(Some(&legacy), &next).is_ok());
    assert_eq!(
        serde_json::to_value(&legacy).unwrap()["execution"][1]["operation"],
        "worker.notify"
    );
    next.delegation.push(legacy.execution[1].clone());
    assert!(validate_managed_subject_permissions(Some(&legacy), &next).is_err());
    next.delegation.pop();
    next.execution[1].resource = PermissionResource::Worker {
        runtime_id: "r".into(),
        worker_id: "two".into(),
    };
    assert!(validate_managed_subject_permissions(Some(&legacy), &next).is_err());
    assert!(validate_managed_subject_permissions(Some(&legacy), &update(vec![], vec![])).is_ok());
}

#[test]
fn catalog_wire_has_no_worker_target_editors_and_no_implicit_feature_grants() {
    let wire = serde_json::to_value(subject_permission_operations()).unwrap();
    let entries = wire.as_array().unwrap();
    let mut names = std::collections::BTreeSet::new();
    for entry in entries {
        assert!(
            names.insert(entry["operation"].as_str().unwrap()),
            "duplicate descriptor: {entry}"
        );
        assert!(!entry["label"].as_str().unwrap().is_empty());
        assert!(!entry["description"].as_str().unwrap().is_empty());
        assert!(
            !entry["resource_kinds"]
                .as_array()
                .unwrap()
                .contains(&json!("worker"))
        );
        if entry["availability"]["kind"] == "available" {
            assert!(!entry["resource_kinds"].as_array().unwrap().is_empty());
        }
    }
    assert!(!names.contains("ticket"));
    assert!(!names.contains("worker.notify"));
    assert!(!names.contains("worker.result"));
    let mcp = entries.iter().find(|d| d["operation"] == "mcp").unwrap();
    assert_eq!(mcp["resource_kinds"], json!(["feature"]));
    let messaging = entries
        .iter()
        .find(|d| d["operation"] == "worker.messaging")
        .unwrap();
    assert_eq!(messaging["availability"]["kind"], "available");
    assert_eq!(messaging["resource_kinds"], json!(["worker_operation"]));
}

#[test]
fn managed_worker_operations_accept_only_current_operation_use_in_each_lane() {
    for name in [
        "worker.messaging",
        "worker.send_input",
        "worker.cancel",
        "worker.stop",
        "worker.restore",
        "worker.remove",
        "worker.observe",
    ] {
        let p = permission(name, json!({"kind":"worker_operation","workspace_id":"w"}));
        assert!(
            validate_managed_subject_permissions(None, &update(vec![p.clone()], vec![p])).is_ok()
        );
    }
    for name in ["worker.notify", "worker.result", "worker.future"] {
        let p = permission(name, json!({"kind":"worker_operation","workspace_id":"w"}));
        assert!(validate_managed_subject_permissions(None, &update(vec![p], vec![])).is_err());
    }
}
