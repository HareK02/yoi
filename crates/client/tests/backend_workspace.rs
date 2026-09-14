use client::{
    BackendTarget, CreateBackendWorkspaceRepository, CreateBackendWorkspaceRequest, Target,
    WorkerConnectionSelector,
};

#[test]
fn workspace_creation_request_preserves_operation_id_for_retry() {
    let request = CreateBackendWorkspaceRequest {
        operation_id: "workspace-create-1".to_string(),
        display_name: "Alpha".to_string(),
        repository: CreateBackendWorkspaceRepository {
            repository_key: "main".to_string(),
            source: "/srv/repos/alpha".to_string(),
            default_ref: Some("develop".to_string()),
        },
    };

    assert_eq!(request.clone(), request);
    assert_eq!(request.operation_id, "workspace-create-1");
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["operation_id"], "workspace-create-1");
    assert_eq!(json["repository"]["repository_key"], "main");
    assert_eq!(json["repository"]["source"], "/srv/repos/alpha");
    assert!(json.get("operation_key").is_none());
    assert!(json["repository"].get("display_name").is_none());
    assert!(json["repository"].get("uri").is_none());
}

#[test]
fn backend_worker_connection_requires_explicit_workspace_scope() {
    let target = BackendTarget::new("http://127.0.0.1:8787", None::<String>);
    let error = match target.connect_worker(WorkerConnectionSelector::new("runtime-a", "worker-a"))
    {
        Ok(_) => panic!("unscoped Backend worker connection must fail"),
        Err(error) => error,
    };

    assert!(
        error
            .to_string()
            .contains("workspace selection is required")
    );
}
