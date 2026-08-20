use client::{
    BackendTarget, CreateBackendWorkspaceRepository, CreateBackendWorkspaceRequest, Target,
    WorkerConnectionSelector,
};

#[test]
fn workspace_creation_request_preserves_operation_key_for_retry() {
    let request = CreateBackendWorkspaceRequest {
        operation_key: "workspace-create-1".to_string(),
        display_name: "Alpha".to_string(),
        repository: CreateBackendWorkspaceRepository {
            uri: "/srv/repos/alpha".to_string(),
            display_name: Some("Main".to_string()),
            default_ref: Some("develop".to_string()),
        },
    };

    assert_eq!(request.clone(), request);
    assert_eq!(request.operation_key, "workspace-create-1");
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
