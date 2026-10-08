fn root_reference(name: &str) -> wip_protocol::InterfaceReference {
    wip_protocol::InterfaceReference {
        scope: "/".into(),
        name: name.into(),
    }
}

use std::collections::BTreeMap;

use wip_client::{
    CallOutcome, Client, ClientErrorKind, ClientLimits, Completion, DispatchedTransportFailure,
    ObservationState, OutcomeUnknownReason, SecurityContext,
};
use wip_http::http::Response;
use wip_http::{
    EncodeHttpError, Limits, encode_call_operation_response, encode_fetch_interface_response,
    encode_observe_response, encode_protocol_error_response,
};
use wip_protocol::{
    CallOperationResponse, FetchInterfaceRequest, FetchInterfaceResponse, INTERFACE_FORMAT_V1,
    InterfaceDescriptor, Object, ObjectObservation, ObserveRequest, OperationDeclaration,
    ParameterDeclaration, ProtocolError, ProtocolErrorCode, ProtocolInteraction, ReturnDeclaration,
    TypeExpr, Value,
};

fn wire_limits() -> Limits {
    Limits::new(64 * 1024, 64 * 1024, 64).unwrap()
}

#[derive(Clone)]
struct TestObservation {
    object: Object,
    children: Vec<TestObservation>,
}

struct TestObservationResponse {
    root: TestObservation,
}

fn protocol_observation(node: &TestObservation, depth: u32) -> ObjectObservation {
    ObjectObservation {
        object: node.object.clone(),
        children: (depth > 0).then(|| {
            node.children
                .iter()
                .map(|child| protocol_observation(child, depth - 1))
                .collect()
        }),
    }
}

fn encode_test_observation_response(
    request: &ObserveRequest,
    value: &TestObservationResponse,
    limits: Limits,
) -> Result<Response<Vec<u8>>, EncodeHttpError> {
    encode_observe_response(
        request,
        &protocol_observation(&value.root, request.depth),
        limits,
    )
}

fn client(objects: usize, interfaces: usize, in_flight: usize, history: usize) -> Client {
    Client::new(
        ClientLimits::new(8, objects, interfaces, in_flight, history).unwrap(),
        wire_limits(),
    )
}

fn object(name: &str, reference: Option<&str>, validator: &[u8]) -> Object {
    Object {
        name: name.into(),
        description: Some(format!("object {name}")),
        interfaces: vec![root_reference("math")],
        r#ref: reference.map(str::to_owned),
        validator: Some(validator.to_vec()),
    }
}

fn descriptor(return_type: TypeExpr) -> InterfaceDescriptor {
    InterfaceDescriptor {
        format: INTERFACE_FORMAT_V1.into(),
        documentation: None,
        types: vec![],
        operations: vec![OperationDeclaration {
            name: "calculate".into(),
            documentation: None,
            parameters: vec![ParameterDeclaration {
                name: "value".into(),
                required: true,
                documentation: None,
                r#type: TypeExpr::Integer,
            }],
            returns: ReturnDeclaration {
                documentation: None,
                r#type: return_type,
            },
        }],
    }
}

fn observation_response(path: &str, value: &Object) -> Response<Vec<u8>> {
    encode_observe_response(
        &ObserveRequest {
            path: path.into(),
            depth: 0,
        },
        &ObjectObservation {
            object: value.clone(),
            children: None,
        },
        wire_limits(),
    )
    .unwrap()
}

fn observe_object(client: &mut Client, session: &wip_client::SessionId, path: &str, value: Object) {
    let request = client.refresh_observed(session, path, 0).unwrap();
    client
        .complete(request.id, observation_response(path, &value))
        .unwrap();
}

fn observe_interface(
    client: &mut Client,
    session: &wip_client::SessionId,
    value: FetchInterfaceResponse,
) {
    let request = client
        .prepare_interface(session, value.interface.clone())
        .unwrap();
    let response = encode_fetch_interface_response(
        &FetchInterfaceRequest {
            interface: value.interface.clone(),
        },
        &value,
        wire_limits(),
    )
    .unwrap();
    client.complete(request.id, response).unwrap();
}

fn args(value: i64) -> BTreeMap<String, Value> {
    BTreeMap::from([("value".into(), Value::Integer(value))])
}

#[test]
fn sessions_partition_known_space_by_canonical_endpoint_and_security_context() {
    let mut client = client(8, 8, 8, 8);
    let alice = client
        .open_session(
            "https://example.test/wip",
            SecurityContext::new("alice-token"),
        )
        .unwrap();
    let alice_canonical = client
        .open_session(
            "https://example.test/wip/",
            SecurityContext::new("alice-token"),
        )
        .unwrap();
    let bob = client
        .open_session(
            "https://example.test/wip",
            SecurityContext::new("bob-token"),
        )
        .unwrap();
    let other = client
        .open_session(
            "https://other.test/wip",
            SecurityContext::new("alice-token"),
        )
        .unwrap();

    assert_eq!(alice, alice_canonical);
    observe_object(&mut client, &alice, "/item", object("item", None, b"a"));
    assert!(client.object(&alice, "/item").is_some());
    assert!(client.object(&bob, "/item").is_none());
    assert!(client.object(&other, "/item").is_none());
}

#[test]
fn ensure_observed_deduplicates_loading_and_fresh_coverage() {
    let mut client = client(8, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    let request = ObserveRequest {
        path: "/".into(),
        depth: 1,
    };
    let value = TestObservationResponse {
        root: TestObservation {
            object: object("", None, b"root"),
            children: vec![TestObservation {
                object: object("child", None, b"child"),
                children: vec![],
            }],
        },
    };

    let root = client
        .ensure_observed(&session, "/", 1)
        .unwrap()
        .expect("missing coverage starts an observation");
    assert!(client.ensure_observed(&session, "/", 1).unwrap().is_none());
    client
        .complete(
            root.id,
            encode_test_observation_response(&request, &value, wire_limits()).unwrap(),
        )
        .unwrap();
    assert!(client.ensure_observed(&session, "/", 1).unwrap().is_none());

    let child = client
        .ensure_observed(&session, "/child", 1)
        .unwrap()
        .expect("depth-boundary children remain missing coverage");
    assert!(
        client
            .ensure_observed(&session, "/child", 1)
            .unwrap()
            .is_none()
    );
    client
        .complete(
            child.id,
            encode_test_observation_response(
                &ObserveRequest {
                    path: "/child".into(),
                    depth: 1,
                },
                &TestObservationResponse {
                    root: TestObservation {
                        object: object("child", None, b"child"),
                        children: vec![],
                    },
                },
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap();
    assert!(
        client
            .ensure_observed(&session, "/child", 1)
            .unwrap()
            .is_none()
    );
}

#[test]
fn ensure_observed_respects_capacity_and_does_not_loop_on_failure() {
    let mut client = client(8, 8, 1, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    let first = client
        .ensure_observed(&session, "/one", 0)
        .unwrap()
        .expect("first observation fits");
    assert_eq!(
        client
            .ensure_observed(&session, "relative", 0)
            .unwrap_err()
            .kind,
        ClientErrorKind::InvalidRequest
    );
    assert!(
        client
            .ensure_observed(&session, "/two", 0)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        client
            .fail_transport(first.id, "scripted failure")
            .unwrap_err()
            .kind,
        ClientErrorKind::Transport
    );
    assert!(
        client
            .ensure_observed(&session, "/one", 0)
            .unwrap()
            .is_none(),
        "retained failures require an explicit refresh"
    );
    assert!(
        client
            .ensure_observed(&session, "/two", 0)
            .unwrap()
            .is_some()
    );
}

#[test]
fn ensure_interface_deduplicates_loading_fresh_and_retained_failure() {
    let mut client = client(8, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();

    let request = client
        .ensure_interface(&session, root_reference("math"))
        .unwrap()
        .expect("missing descriptor starts a fetch");
    assert!(
        client
            .ensure_interface(&session, root_reference("math"))
            .unwrap()
            .is_none()
    );
    let value = FetchInterfaceResponse {
        scope_ref: None,
        interface: root_reference("math"),
        descriptor: descriptor(TypeExpr::Integer),
        validator: Some(b"math-v1".to_vec()),
    };
    client
        .complete(
            request.id,
            encode_fetch_interface_response(
                &FetchInterfaceRequest {
                    interface: root_reference("math"),
                },
                &value,
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap();
    assert!(
        client
            .ensure_interface(&session, root_reference("math"))
            .unwrap()
            .is_none()
    );

    let failed = client
        .ensure_interface(&session, root_reference("broken"))
        .unwrap()
        .expect("new descriptor starts a fetch");
    assert_eq!(
        client
            .fail_transport(failed.id, "scripted failure")
            .unwrap_err()
            .kind,
        ClientErrorKind::Transport
    );
    assert!(
        client
            .ensure_interface(&session, root_reference("broken"))
            .unwrap()
            .is_none(),
        "retained interface failures require an explicit refresh"
    );
}

#[test]
fn ensure_interface_waits_for_request_and_cache_capacity_without_evicting() {
    let mut client = client(8, 1, 1, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    let pending_object = client.refresh_object(&session, "/busy").unwrap();
    assert!(
        client
            .ensure_interface(&session, root_reference("math"))
            .unwrap()
            .is_none(),
        "lookahead waits while the in-flight bound is full"
    );
    client
        .complete(
            pending_object.id,
            observation_response("/busy", &object("busy", None, b"busy")),
        )
        .unwrap();

    let request = client
        .ensure_interface(&session, root_reference("math"))
        .unwrap()
        .expect("lookahead resumes after capacity becomes available");
    let value = FetchInterfaceResponse {
        scope_ref: None,
        interface: root_reference("math"),
        descriptor: descriptor(TypeExpr::Integer),
        validator: None,
    };
    client
        .complete(
            request.id,
            encode_fetch_interface_response(
                &FetchInterfaceRequest {
                    interface: root_reference("math"),
                },
                &value,
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap();

    assert!(
        client
            .ensure_interface(&session, root_reference("other"))
            .unwrap()
            .is_none(),
        "lookahead does not churn a full descriptor cache"
    );
    assert!(
        client
            .interface(&session, &root_reference("math"))
            .is_some()
    );
    assert!(
        client
            .interface(&session, &root_reference("other"))
            .is_none()
    );

    let explicit = client
        .prepare_interface(&session, root_reference("other"))
        .unwrap();
    assert!(
        client
            .interface(&session, &root_reference("math"))
            .is_none()
    );
    assert!(matches!(
        client.interface(&session, &root_reference("other")).unwrap().state,
        ObservationState::Loading(id) if id == explicit.id
    ));
}

#[test]
fn independent_retrievals_run_concurrently_and_complete_out_of_order() {
    let mut client = client(8, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();

    let first = client.refresh_observed(&session, "/one", 0).unwrap();
    let second = client.refresh_observed(&session, "/two", 0).unwrap();
    let interface = client
        .prepare_interface(&session, root_reference("math"))
        .unwrap();

    assert_eq!(first.request.uri().path(), "/wip/v1/observe");
    assert_eq!(second.request.uri().path(), "/wip/v1/observe");
    assert_eq!(interface.request.uri().path(), "/wip/v1/fetch_interface");
    assert_eq!(client.in_flight_len(), 3);

    client
        .complete(
            second.id,
            observation_response("/two", &object("two", None, b"two")),
        )
        .unwrap();
    client
        .complete(
            interface.id,
            encode_fetch_interface_response(
                &FetchInterfaceRequest {
                    interface: root_reference("math"),
                },
                &FetchInterfaceResponse {
                    scope_ref: None,
                    interface: root_reference("math"),
                    descriptor: descriptor(TypeExpr::Integer),
                    validator: Some(b"math-v1".to_vec()),
                },
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap();
    client
        .complete(
            first.id,
            observation_response("/one", &object("one", None, b"one")),
        )
        .unwrap();

    assert_eq!(
        client.object(&session, "/one").unwrap().state,
        ObservationState::Fresh
    );
    assert_eq!(
        client.object(&session, "/two").unwrap().state,
        ObservationState::Fresh
    );
    assert_eq!(
        client
            .interface(&session, &root_reference("math"))
            .unwrap()
            .state,
        ObservationState::Fresh
    );
    assert_eq!(client.in_flight_len(), 0);
}

#[test]
fn newer_refresh_rejects_out_of_order_response_for_the_same_subject() {
    let mut client = client(8, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    let old = client.refresh_observed(&session, "/item", 0).unwrap();
    let new = client.refresh_observed(&session, "/item", 0).unwrap();

    assert_eq!(
        client
            .complete(
                old.id,
                observation_response("/item", &object("item", None, b"old"))
            )
            .unwrap(),
        Completion::StaleResponseRejected(old.id)
    );
    assert!(matches!(
        client.object(&session, "/item").unwrap().state,
        ObservationState::Loading(id) if id == new.id
    ));

    client
        .complete(
            new.id,
            observation_response("/item", &object("item", None, b"new")),
        )
        .unwrap();
    assert_eq!(
        client.object(&session, "/item").unwrap().validator,
        Some(b"new".to_vec())
    );
}

#[test]
fn normalized_object_cache_rejects_conflicts_and_evicts_to_its_bound() {
    let mut client = client(2, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    observe_object(
        &mut client,
        &session,
        "/a/shared",
        object("shared", Some("shared-ref"), b"v1"),
    );
    observe_object(
        &mut client,
        &session,
        "/b/shared",
        object("shared", Some("shared-ref"), b"v1"),
    );

    let conflict = client.refresh_observed(&session, "/b/shared", 0).unwrap();
    let mut changed = object("shared", Some("shared-ref"), b"v1");
    changed.description = Some("contradiction".into());
    let error = client
        .complete(conflict.id, observation_response("/b/shared", &changed))
        .unwrap_err();
    assert_eq!(error.kind, ClientErrorKind::InvalidResponse);

    observe_object(
        &mut client,
        &session,
        "/second",
        object("second", None, b"2"),
    );
    observe_object(&mut client, &session, "/third", object("third", None, b"3"));
    assert!(client.object(&session, "/a/shared").is_none());
    assert!(client.object(&session, "/second").is_some());
    assert!(client.object(&session, "/third").is_some());
}

#[test]
fn normalized_aliases_preserve_newer_request_ownership() {
    let mut client = client(8, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    observe_object(
        &mut client,
        &session,
        "/a/shared",
        object("shared", Some("shared-ref"), b"initial"),
    );
    observe_object(
        &mut client,
        &session,
        "/b/shared",
        object("shared", Some("shared-ref"), b"initial"),
    );
    let request = ObserveRequest {
        path: "/a".into(),
        depth: 1,
    };
    let older_tree_response = || TestObservationResponse {
        root: TestObservation {
            object: object("a", None, b"root"),
            children: vec![TestObservation {
                object: object("shared", Some("shared-ref"), b"older"),
                children: vec![],
            }],
        },
    };

    let older_tree = client.refresh_observed(&session, "/a", 1).unwrap();
    let newer_alias = client.refresh_observed(&session, "/b/shared", 0).unwrap();
    client
        .complete(
            newer_alias.id,
            observation_response("/b/shared", &object("shared", Some("shared-ref"), b"newer")),
        )
        .unwrap();
    client
        .complete(
            older_tree.id,
            encode_test_observation_response(&request, &older_tree_response(), wire_limits())
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        client.object(&session, "/a/shared").unwrap().validator,
        Some(b"newer".to_vec())
    );
    assert_eq!(
        client.object(&session, "/b/shared").unwrap().validator,
        Some(b"newer".to_vec())
    );

    let older_tree = client.refresh_observed(&session, "/a", 1).unwrap();
    let newer_alias = client.refresh_observed(&session, "/b/shared", 0).unwrap();
    client
        .complete(
            older_tree.id,
            encode_test_observation_response(&request, &older_tree_response(), wire_limits())
                .unwrap(),
        )
        .unwrap();
    assert!(matches!(
        client.object(&session, "/b/shared").unwrap().state,
        ObservationState::Loading(id) if id == newer_alias.id
    ));
    assert_eq!(
        client.object(&session, "/b/shared").unwrap().validator,
        Some(b"newer".to_vec())
    );
    client
        .complete(
            newer_alias.id,
            observation_response(
                "/b/shared",
                &object("shared", Some("shared-ref"), b"newest"),
            ),
        )
        .unwrap();
    assert_eq!(
        client.object(&session, "/a/shared").unwrap().validator,
        Some(b"newest".to_vec())
    );

    let older_tree = client.refresh_observed(&session, "/a", 1).unwrap();
    let newer_alias = client.refresh_observed(&session, "/b/shared", 0).unwrap();
    client
        .complete(
            newer_alias.id,
            observation_response(
                "/b/shared",
                &object("shared", Some("shared-ref"), b"newest"),
            ),
        )
        .unwrap();
    let mut contradictory = object("shared", Some("shared-ref"), b"newest");
    contradictory.description = Some("contradictory representation".into());
    let error = client
        .complete(
            older_tree.id,
            encode_test_observation_response(
                &request,
                &TestObservationResponse {
                    root: TestObservation {
                        object: object("a", None, b"root"),
                        children: vec![TestObservation {
                            object: contradictory,
                            children: vec![],
                        }],
                    },
                },
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap_err();
    assert_eq!(error.kind, ClientErrorKind::InvalidResponse);
    assert_eq!(
        client.object(&session, "/b/shared").unwrap().validator,
        Some(b"newest".to_vec())
    );

    observe_object(
        &mut client,
        &session,
        "/child",
        object("child", Some("child-ref"), b"stable"),
    );
    let older_tree = client.refresh_observed(&session, "/", 1).unwrap();
    let newer_child = client.refresh_observed(&session, "/child", 0).unwrap();
    client
        .complete(
            newer_child.id,
            observation_response("/child", &object("child", Some("child-ref"), b"stable")),
        )
        .unwrap();
    let mut contradictory = object("child", Some("child-ref"), b"stable");
    contradictory.description = Some("same-path contradiction".into());
    let root_request = ObserveRequest {
        path: "/".into(),
        depth: 1,
    };
    let error = client
        .complete(
            older_tree.id,
            encode_test_observation_response(
                &root_request,
                &TestObservationResponse {
                    root: TestObservation {
                        object: object("", None, b"root"),
                        children: vec![TestObservation {
                            object: contradictory,
                            children: vec![],
                        }],
                    },
                },
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap_err();
    assert_eq!(error.kind, ClientErrorKind::InvalidResponse);
    assert_eq!(
        client.object(&session, "/child").unwrap().state,
        ObservationState::Fresh
    );
}

#[test]
fn observation_expansion_is_complete_and_refreshes_edges() {
    let mut client = client(16, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    assert!(client.tree(&session, "/").is_none());

    let request = client.refresh_observed(&session, "/", 1).unwrap();
    let logical_request = ObserveRequest {
        path: "/".into(),
        depth: 1,
    };
    let tree = TestObservationResponse {
        root: TestObservation {
            object: object("", None, b"root"),
            children: vec![TestObservation {
                object: object("items", None, b"items"),
                children: vec![],
            }],
        },
    };
    client
        .complete(
            request.id,
            encode_test_observation_response(&logical_request, &tree, wire_limits()).unwrap(),
        )
        .unwrap();
    assert_eq!(client.tree(&session, "/").unwrap().children, ["/items"]);
    assert!(client.object(&session, "/items").is_some());

    let refresh = client.refresh_observed(&session, "/", 1).unwrap();
    let refreshed = TestObservationResponse {
        root: TestObservation {
            object: object("", None, b"root-2"),
            children: vec![],
        },
    };
    client
        .complete(
            refresh.id,
            encode_test_observation_response(&logical_request, &refreshed, wire_limits()).unwrap(),
        )
        .unwrap();
    assert!(client.tree(&session, "/").unwrap().children.is_empty());
    assert_eq!(
        client.object(&session, "/items").unwrap().state,
        ObservationState::Stale
    );
}

#[test]
fn depth_zero_observation_preserves_previously_observed_children() {
    let mut client = client(16, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    let deep_request = ObserveRequest {
        path: "/".into(),
        depth: 1,
    };
    let deep = client.refresh_observed(&session, "/", 1).unwrap();
    client
        .complete(
            deep.id,
            encode_test_observation_response(
                &deep_request,
                &TestObservationResponse {
                    root: TestObservation {
                        object: object("", None, b"root-v1"),
                        children: vec![TestObservation {
                            object: object("child", None, b"child-v1"),
                            children: vec![],
                        }],
                    },
                },
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap();

    let shallow = client.refresh_observed(&session, "/", 0).unwrap();
    client
        .complete(
            shallow.id,
            observation_response("/", &object("", None, b"root-v2")),
        )
        .unwrap();

    assert_eq!(client.tree(&session, "/").unwrap().children, ["/child"]);
    assert_eq!(
        client.object(&session, "/child").unwrap().state,
        ObservationState::Fresh
    );
    assert_eq!(
        client.object(&session, "/").unwrap().validator,
        Some(b"root-v2".to_vec())
    );
}

#[test]
fn shallow_observation_supersedes_loading_deep_observation_without_leaking_loading_state() {
    let mut client = client(16, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    let deep_request = ObserveRequest {
        path: "/".into(),
        depth: 1,
    };
    let deep_value = TestObservationResponse {
        root: TestObservation {
            object: object("", None, b"old-root"),
            children: vec![],
        },
    };

    let deep = client.refresh_observed(&session, "/", 1).unwrap();
    let shallow = client.refresh_observed(&session, "/", 0).unwrap();
    assert_eq!(
        client.tree(&session, "/").unwrap().state,
        ObservationState::Stale
    );
    assert_eq!(
        client
            .complete(
                deep.id,
                encode_test_observation_response(&deep_request, &deep_value, wire_limits())
                    .unwrap(),
            )
            .unwrap(),
        Completion::StaleResponseRejected(deep.id)
    );
    client
        .complete(
            shallow.id,
            observation_response("/", &object("", None, b"new-root")),
        )
        .unwrap();
    assert_eq!(
        client.tree(&session, "/").unwrap().state,
        ObservationState::Stale
    );
}

#[test]
fn newer_parent_boundary_observation_replans_superseded_child_lookahead() {
    let mut client = client(16, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    let root_request = ObserveRequest {
        path: "/".into(),
        depth: 1,
    };
    let root_value = |validator: &'static [u8]| TestObservationResponse {
        root: TestObservation {
            object: object("", None, validator),
            children: vec![TestObservation {
                object: object("child", None, validator),
                children: vec![],
            }],
        },
    };
    let initial = client.refresh_observed(&session, "/", 1).unwrap();
    client
        .complete(
            initial.id,
            encode_test_observation_response(&root_request, &root_value(b"initial"), wire_limits())
                .unwrap(),
        )
        .unwrap();

    let older_child = client.refresh_observed(&session, "/child", 1).unwrap();
    let newer_parent = client.refresh_observed(&session, "/", 1).unwrap();
    client
        .complete(
            newer_parent.id,
            encode_test_observation_response(&root_request, &root_value(b"newer"), wire_limits())
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        client.tree(&session, "/child").unwrap().state,
        ObservationState::Stale
    );

    let replanned = client
        .ensure_observed(&session, "/child", 1)
        .unwrap()
        .expect("superseded lookahead is missing rather than permanently loading");
    assert_eq!(
        client
            .complete(
                older_child.id,
                encode_test_observation_response(
                    &ObserveRequest {
                        path: "/child".into(),
                        depth: 1,
                    },
                    &TestObservationResponse {
                        root: TestObservation {
                            object: object("child", None, b"older"),
                            children: vec![],
                        },
                    },
                    wire_limits(),
                )
                .unwrap(),
            )
            .unwrap(),
        Completion::StaleResponseRejected(older_child.id)
    );
    assert!(matches!(
        client.object(&session, "/child").unwrap().state,
        ObservationState::Loading(id) if id == replanned.id
    ));
}

#[test]
fn call_context_fixes_membership_validators_descriptor_and_result_validation() {
    let mut client = client(8, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    observe_object(
        &mut client,
        &session,
        "/item",
        object("item", Some("item-ref"), b"object-v1"),
    );
    observe_interface(
        &mut client,
        &session,
        FetchInterfaceResponse {
            scope_ref: None,
            interface: root_reference("math"),
            descriptor: descriptor(TypeExpr::Integer),
            validator: Some(b"interface-v1".to_vec()),
        },
    );

    let call = client
        .prepare_call(
            &session,
            "/item",
            &root_reference("math"),
            "calculate",
            args(7),
        )
        .unwrap();
    assert_eq!(
        client
            .pending_call_context(call.id)
            .unwrap()
            .operation()
            .name,
        "calculate"
    );
    client.mark_dispatched(call.id).unwrap();

    // A newer interface observation cannot change this in-flight call's schema.
    observe_interface(
        &mut client,
        &session,
        FetchInterfaceResponse {
            scope_ref: None,
            interface: root_reference("math"),
            descriptor: descriptor(TypeExpr::String),
            validator: Some(b"interface-v2".to_vec()),
        },
    );

    let history_context = match client
        .complete(
            call.id,
            encode_call_operation_response(
                &wip_protocol::CallOperationRequest {
                    target: wip_protocol::Target {
                        path: "/item".into(),
                        validator: Some(b"object-v1".to_vec()),
                    },
                    interface: wip_protocol::InterfaceTarget {
                        scope_ref: None,
                        reference: root_reference("math"),
                        validator: Some(b"interface-v1".to_vec()),
                    },
                    operation: "calculate".into(),
                    arguments: args(7),
                },
                &descriptor(TypeExpr::Integer),
                &CallOperationResponse {
                    result: Value::Integer(14),
                    validator: Some(b"object-v2".to_vec()),
                },
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap()
    {
        Completion::Call(record) => record,
        other => panic!("unexpected completion: {other:?}"),
    };
    assert_eq!(
        history_context.context.descriptor(),
        &descriptor(TypeExpr::Integer)
    );
    assert_eq!(
        history_context.context.object().validator,
        Some(b"object-v1".to_vec())
    );
    assert_eq!(
        history_context.context.interface().validator,
        Some(b"interface-v1".to_vec())
    );
    assert!(matches!(history_context.outcome, CallOutcome::Success(_)));
    assert_eq!(
        client
            .interface(&session, &root_reference("math"))
            .unwrap()
            .descriptor,
        Some(descriptor(TypeExpr::String))
    );
    let observation = client.object(&session, "/item").unwrap();
    assert_eq!(observation.state, ObservationState::Fresh);
    assert_eq!(observation.validator, Some(b"object-v2".to_vec()));
}

#[test]
fn successful_call_without_validator_preserves_observed_validator() {
    let mut client = client(8, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    observe_object(
        &mut client,
        &session,
        "/item",
        object("item", None, b"object-v1"),
    );
    observe_interface(
        &mut client,
        &session,
        FetchInterfaceResponse {
            scope_ref: None,
            interface: root_reference("math"),
            descriptor: descriptor(TypeExpr::Integer),
            validator: None,
        },
    );

    let call = client
        .prepare_call(
            &session,
            "/item",
            &root_reference("math"),
            "calculate",
            args(1),
        )
        .unwrap();
    let request = client
        .pending_call_context(call.id)
        .unwrap()
        .request()
        .clone();
    let fixed_descriptor = client
        .pending_call_context(call.id)
        .unwrap()
        .descriptor()
        .clone();
    client.mark_dispatched(call.id).unwrap();
    client
        .complete(
            call.id,
            encode_call_operation_response(
                &request,
                &fixed_descriptor,
                &CallOperationResponse {
                    result: Value::Integer(2),
                    validator: None,
                },
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap();

    let observation = client.object(&session, "/item").unwrap();
    assert_eq!(observation.state, ObservationState::Fresh);
    assert_eq!(observation.validator, Some(b"object-v1".to_vec()));

    let next = client
        .prepare_call(
            &session,
            "/item",
            &root_reference("math"),
            "calculate",
            args(2),
        )
        .unwrap();
    assert_eq!(
        client
            .pending_call_context(next.id)
            .unwrap()
            .request()
            .target
            .validator,
        Some(b"object-v1".to_vec())
    );
}

#[test]
fn newer_observation_prevents_older_success_validator_overwrite() {
    let mut client = client(8, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    observe_object(
        &mut client,
        &session,
        "/item",
        object("item", None, b"object-v1"),
    );
    observe_interface(
        &mut client,
        &session,
        FetchInterfaceResponse {
            scope_ref: None,
            interface: root_reference("math"),
            descriptor: descriptor(TypeExpr::Integer),
            validator: None,
        },
    );

    let call = client
        .prepare_call(
            &session,
            "/item",
            &root_reference("math"),
            "calculate",
            args(1),
        )
        .unwrap();
    let request = client
        .pending_call_context(call.id)
        .unwrap()
        .request()
        .clone();
    let fixed_descriptor = client
        .pending_call_context(call.id)
        .unwrap()
        .descriptor()
        .clone();
    client.mark_dispatched(call.id).unwrap();

    observe_object(
        &mut client,
        &session,
        "/item",
        object("item", None, b"object-v3"),
    );
    client
        .complete(
            call.id,
            encode_call_operation_response(
                &request,
                &fixed_descriptor,
                &CallOperationResponse {
                    result: Value::Integer(2),
                    validator: Some(b"object-v2".to_vec()),
                },
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap();

    let observation = client.object(&session, "/item").unwrap();
    assert_eq!(observation.state, ObservationState::Fresh);
    assert_eq!(observation.validator, Some(b"object-v3".to_vec()));
}

#[test]
fn older_retrieval_cannot_overwrite_success_validator() {
    let mut client = client(8, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    observe_object(
        &mut client,
        &session,
        "/item",
        object("item", Some("shared-ref"), b"object-v1"),
    );
    observe_interface(
        &mut client,
        &session,
        FetchInterfaceResponse {
            scope_ref: None,
            interface: root_reference("math"),
            descriptor: descriptor(TypeExpr::Integer),
            validator: Some(b"interface-v1".to_vec()),
        },
    );

    let older_alias = client.refresh_observed(&session, "/alias/item", 0).unwrap();
    let call = client
        .prepare_call(
            &session,
            "/item",
            &root_reference("math"),
            "calculate",
            args(7),
        )
        .unwrap();
    let request = client
        .pending_call_context(call.id)
        .unwrap()
        .request()
        .clone();
    let fixed_descriptor = client
        .pending_call_context(call.id)
        .unwrap()
        .descriptor()
        .clone();
    client.mark_dispatched(call.id).unwrap();
    client
        .complete(
            call.id,
            encode_call_operation_response(
                &request,
                &fixed_descriptor,
                &CallOperationResponse {
                    result: Value::Integer(14),
                    validator: Some(b"object-v2".to_vec()),
                },
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap();
    let current = client.object(&session, "/item").unwrap();
    assert_eq!(current.state, ObservationState::Fresh);
    assert_eq!(current.validator, Some(b"object-v2".to_vec()));

    client
        .complete(
            older_alias.id,
            observation_response(
                "/alias/item",
                &object("item", Some("shared-ref"), b"object-v1"),
            ),
        )
        .unwrap();
    let alias = client.object(&session, "/alias/item").unwrap();
    assert_eq!(alias.state, ObservationState::Stale);
    assert_eq!(alias.validator, Some(b"object-v2".to_vec()));
    assert_eq!(
        client
            .prepare_call(
                &session,
                "/alias/item",
                &root_reference("math"),
                "calculate",
                args(1)
            )
            .unwrap_err()
            .kind,
        ClientErrorKind::MissingObservation
    );

    observe_object(
        &mut client,
        &session,
        "/item",
        object("item", Some("shared-ref"), b"object-v2"),
    );
    let older_alias = client
        .refresh_observed(&session, "/second/item", 0)
        .unwrap();
    let call = client
        .prepare_call(
            &session,
            "/item",
            &root_reference("math"),
            "calculate",
            args(8),
        )
        .unwrap();
    client.mark_dispatched(call.id).unwrap();
    let unknown = ProtocolError {
        code: ProtocolErrorCode::OperationOutcomeUnknown,
        message: "commit uncertain".into(),
    };
    client
        .complete(
            call.id,
            encode_protocol_error_response(
                ProtocolInteraction::CallOperation,
                &unknown,
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap();
    client
        .complete(
            older_alias.id,
            observation_response(
                "/second/item",
                &object("item", Some("shared-ref"), b"object-v2"),
            ),
        )
        .unwrap();
    assert_eq!(
        client.object(&session, "/second/item").unwrap().state,
        ObservationState::Stale
    );
}

#[test]
fn in_flight_calls_pin_target_identity_until_staleness_is_applied() {
    let mut client = client(1, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    observe_object(
        &mut client,
        &session,
        "/item",
        object("item", Some("shared-ref"), b"object-v1"),
    );
    observe_interface(
        &mut client,
        &session,
        FetchInterfaceResponse {
            scope_ref: None,
            interface: root_reference("math"),
            descriptor: descriptor(TypeExpr::Integer),
            validator: Some(b"interface-v1".to_vec()),
        },
    );
    let call = client
        .prepare_call(
            &session,
            "/item",
            &root_reference("math"),
            "calculate",
            args(7),
        )
        .unwrap();
    client.mark_dispatched(call.id).unwrap();

    assert_eq!(
        client
            .refresh_observed(&session, "/other", 0)
            .unwrap_err()
            .kind,
        ClientErrorKind::Capacity
    );
    let unknown = ProtocolError {
        code: ProtocolErrorCode::OperationOutcomeUnknown,
        message: "commit uncertain".into(),
    };
    client
        .complete(
            call.id,
            encode_protocol_error_response(
                ProtocolInteraction::CallOperation,
                &unknown,
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        client.object(&session, "/item").unwrap().state,
        ObservationState::Stale
    );

    observe_object(
        &mut client,
        &session,
        "/other",
        object("other", None, b"other"),
    );
    assert!(client.object(&session, "/item").is_none());
    assert!(client.object(&session, "/other").is_some());
}

#[test]
fn validator_mismatch_stales_only_its_observation_and_never_retries() {
    let mut client = client(8, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    observe_object(&mut client, &session, "/item", object("item", None, b"v1"));
    observe_interface(
        &mut client,
        &session,
        FetchInterfaceResponse {
            scope_ref: None,
            interface: root_reference("math"),
            descriptor: descriptor(TypeExpr::Integer),
            validator: Some(b"i1".to_vec()),
        },
    );
    let call = client
        .prepare_call(
            &session,
            "/item",
            &root_reference("math"),
            "calculate",
            args(1),
        )
        .unwrap();
    client.mark_dispatched(call.id).unwrap();
    let error = ProtocolError {
        code: ProtocolErrorCode::InterfaceValidatorMismatch,
        message: "stale descriptor".into(),
    };
    let response =
        encode_protocol_error_response(ProtocolInteraction::CallOperation, &error, wire_limits())
            .unwrap();
    client.complete(call.id, response).unwrap();

    assert_eq!(
        client
            .interface(&session, &root_reference("math"))
            .unwrap()
            .state,
        ObservationState::Stale
    );
    assert_eq!(
        client.object(&session, "/item").unwrap().state,
        ObservationState::Fresh
    );
    assert_eq!(client.in_flight_len(), 0);
    assert_eq!(client.call_history(&session).unwrap().len(), 1);

    // Reobservation changes cache state but does not manufacture a replacement call.
    observe_interface(
        &mut client,
        &session,
        FetchInterfaceResponse {
            scope_ref: None,
            interface: root_reference("math"),
            descriptor: descriptor(TypeExpr::Integer),
            validator: Some(b"i2".to_vec()),
        },
    );
    assert_eq!(client.in_flight_len(), 0);
    assert_eq!(client.call_history(&session).unwrap().len(), 1);
}

#[test]
fn protocol_and_transport_unknown_outcomes_stale_without_retry() {
    let mut client = client(8, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    observe_object(&mut client, &session, "/item", object("item", None, b"v1"));
    observe_interface(
        &mut client,
        &session,
        FetchInterfaceResponse {
            scope_ref: None,
            interface: root_reference("math"),
            descriptor: descriptor(TypeExpr::Integer),
            validator: None,
        },
    );

    let call = client
        .prepare_call(
            &session,
            "/item",
            &root_reference("math"),
            "calculate",
            args(1),
        )
        .unwrap();
    client.mark_dispatched(call.id).unwrap();
    let unknown = ProtocolError {
        code: ProtocolErrorCode::OperationOutcomeUnknown,
        message: "commit uncertain".into(),
    };
    let completion = client
        .complete(
            call.id,
            encode_protocol_error_response(
                ProtocolInteraction::CallOperation,
                &unknown,
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap();
    assert!(matches!(
        completion,
        Completion::Call(record)
            if matches!(
                record.outcome,
                CallOutcome::Unknown {
                    reason: OutcomeUnknownReason::Protocol,
                    ..
                }
            )
    ));
    assert_eq!(
        client.object(&session, "/item").unwrap().state,
        ObservationState::Stale
    );

    observe_object(&mut client, &session, "/item", object("item", None, b"v2"));
    let second = client
        .prepare_call(
            &session,
            "/item",
            &root_reference("math"),
            "calculate",
            args(2),
        )
        .unwrap();
    client.mark_dispatched(second.id).unwrap();
    let completion = client
        .fail_dispatched_call(
            second.id,
            DispatchedTransportFailure::Timeout,
            "deadline elapsed",
        )
        .unwrap();
    assert!(matches!(
        completion,
        Completion::Call(record)
            if matches!(
                record.outcome,
                CallOutcome::Unknown {
                    reason: OutcomeUnknownReason::Timeout,
                    ..
                }
            )
    ));
    assert_eq!(client.in_flight_len(), 0);
    assert_eq!(client.call_history(&session).unwrap().len(), 2);
}

#[test]
fn overlapping_tree_work_preserves_newer_subject_request_ownership() {
    let mut client = client(8, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    let tree_request = ObserveRequest {
        path: "/".into(),
        depth: 1,
    };
    let tree_value = TestObservationResponse {
        root: TestObservation {
            object: object("", None, b"root"),
            children: vec![TestObservation {
                object: object("child", None, b"tree-child"),
                children: vec![],
            }],
        },
    };

    let old_tree = client.refresh_observed(&session, "/", 1).unwrap();
    let newer_observation = client.refresh_observed(&session, "/child", 0).unwrap();
    client
        .complete(
            old_tree.id,
            encode_test_observation_response(&tree_request, &tree_value, wire_limits()).unwrap(),
        )
        .unwrap();
    assert!(matches!(
        client.object(&session, "/child").unwrap().state,
        ObservationState::Loading(id) if id == newer_observation.id
    ));
    client
        .complete(
            newer_observation.id,
            observation_response("/child", &object("child", None, b"newer-observation")),
        )
        .unwrap();
    assert_eq!(
        client.object(&session, "/child").unwrap().validator,
        Some(b"newer-observation".to_vec())
    );

    let older_tree = client.refresh_observed(&session, "/", 1).unwrap();
    let completed_newer_observation = client.refresh_observed(&session, "/child", 0).unwrap();
    client
        .complete(
            completed_newer_observation.id,
            observation_response("/child", &object("child", None, b"completed-newer")),
        )
        .unwrap();
    client
        .complete(
            older_tree.id,
            encode_test_observation_response(&tree_request, &tree_value, wire_limits()).unwrap(),
        )
        .unwrap();
    assert_eq!(
        client.object(&session, "/child").unwrap().validator,
        Some(b"completed-newer".to_vec())
    );

    let older_omitting_tree = client.refresh_observed(&session, "/", 1).unwrap();
    let newer_observation = client.refresh_observed(&session, "/child", 0).unwrap();
    client
        .complete(
            newer_observation.id,
            observation_response("/child", &object("child", None, b"newer-than-omission")),
        )
        .unwrap();
    client
        .complete(
            older_omitting_tree.id,
            encode_test_observation_response(
                &tree_request,
                &TestObservationResponse {
                    root: TestObservation {
                        object: object("", None, b"older-root"),
                        children: vec![],
                    },
                },
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap();
    let child = client.object(&session, "/child").unwrap();
    assert_eq!(child.state, ObservationState::Fresh);
    assert_eq!(child.validator, Some(b"newer-than-omission".to_vec()));

    let old_tree = client.refresh_observed(&session, "/", 1).unwrap();
    let newer_child_tree = client.refresh_observed(&session, "/child", 1).unwrap();
    client
        .complete(
            old_tree.id,
            encode_test_observation_response(&tree_request, &tree_value, wire_limits()).unwrap(),
        )
        .unwrap();
    assert!(matches!(
        client.tree(&session, "/child").unwrap().state,
        ObservationState::Loading(id) if id == newer_child_tree.id
    ));
    let child_observation = client.object(&session, "/child").unwrap();
    assert!(matches!(
        child_observation.state,
        ObservationState::Loading(id) if id == newer_child_tree.id
    ));
    assert_eq!(
        child_observation.validator,
        Some(b"newer-than-omission".to_vec())
    );
    let child_request = ObserveRequest {
        path: "/child".into(),
        depth: 1,
    };
    let child_value = TestObservationResponse {
        root: TestObservation {
            object: object("child", None, b"newer-tree"),
            children: vec![],
        },
    };
    client
        .complete(
            newer_child_tree.id,
            encode_test_observation_response(&child_request, &child_value, wire_limits()).unwrap(),
        )
        .unwrap();
    assert_eq!(
        client.tree(&session, "/child").unwrap().state,
        ObservationState::Fresh
    );

    let older_parent = client.refresh_observed(&session, "/", 1).unwrap();
    let newer_child = client.refresh_observed(&session, "/child", 1).unwrap();
    client
        .complete(
            newer_child.id,
            encode_test_observation_response(
                &child_request,
                &TestObservationResponse {
                    root: TestObservation {
                        object: object("child", None, b"completed-newer-tree"),
                        children: vec![],
                    },
                },
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap();
    client
        .complete(
            older_parent.id,
            encode_test_observation_response(&tree_request, &tree_value, wire_limits()).unwrap(),
        )
        .unwrap();
    assert_eq!(
        client.object(&session, "/child").unwrap().validator,
        Some(b"completed-newer-tree".to_vec())
    );
    assert_eq!(
        client.tree(&session, "/child").unwrap().revision,
        newer_child.id
    );
}

#[test]
fn superseded_transport_failure_does_not_poison_newer_refresh() {
    let mut client = client(8, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    let old = client.refresh_observed(&session, "/item", 0).unwrap();
    let newer = client.refresh_observed(&session, "/item", 0).unwrap();
    assert_eq!(
        client
            .fail_transport(old.id, "old connection failed")
            .unwrap(),
        Completion::StaleResponseRejected(old.id)
    );
    assert!(matches!(
        client.object(&session, "/item").unwrap().state,
        ObservationState::Loading(id) if id == newer.id
    ));
    client
        .complete(
            newer.id,
            observation_response("/item", &object("item", None, b"new")),
        )
        .unwrap();

    let old = client.refresh_observed(&session, "/", 1).unwrap();
    let newer_tree = client.refresh_observed(&session, "/", 1).unwrap();
    assert_eq!(
        client
            .fail_transport(old.id, "old tree connection failed")
            .unwrap(),
        Completion::StaleResponseRejected(old.id)
    );
    assert!(matches!(
        client.tree(&session, "/").unwrap().state,
        ObservationState::Loading(id) if id == newer_tree.id
    ));
    client
        .complete(
            newer_tree.id,
            encode_test_observation_response(
                &ObserveRequest {
                    path: "/".into(),
                    depth: 1,
                },
                &TestObservationResponse {
                    root: TestObservation {
                        object: object("", None, b"root"),
                        children: vec![],
                    },
                },
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap();

    let old = client
        .prepare_interface(&session, root_reference("math"))
        .unwrap();
    let newer = client
        .prepare_interface(&session, root_reference("math"))
        .unwrap();
    assert_eq!(
        client
            .fail_transport(old.id, "old connection failed")
            .unwrap(),
        Completion::StaleResponseRejected(old.id)
    );
    assert!(matches!(
        client.interface(&session, &root_reference("math")).unwrap().state,
        ObservationState::Loading(id) if id == newer.id
    ));
    let value = FetchInterfaceResponse {
        scope_ref: None,
        interface: root_reference("math"),
        descriptor: descriptor(TypeExpr::Integer),
        validator: Some(b"current".to_vec()),
    };
    client
        .complete(
            newer.id,
            encode_fetch_interface_response(
                &FetchInterfaceRequest {
                    interface: root_reference("math"),
                },
                &value,
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap();
}

#[test]
fn validator_free_interface_observation_remains_immutable() {
    let mut client = client(8, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    observe_interface(
        &mut client,
        &session,
        FetchInterfaceResponse {
            scope_ref: None,
            interface: root_reference("math"),
            descriptor: descriptor(TypeExpr::Integer),
            validator: None,
        },
    );

    let refresh = client
        .prepare_interface(&session, root_reference("math"))
        .unwrap();
    let contradictory = FetchInterfaceResponse {
        scope_ref: None,
        interface: root_reference("math"),
        descriptor: descriptor(TypeExpr::Integer),
        validator: Some(b"became-mutable".to_vec()),
    };
    let error = client
        .complete(
            refresh.id,
            encode_fetch_interface_response(
                &FetchInterfaceRequest {
                    interface: root_reference("math"),
                },
                &contradictory,
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap_err();
    assert_eq!(error.kind, ClientErrorKind::InvalidResponse);
    assert_eq!(
        client
            .interface(&session, &root_reference("math"))
            .unwrap()
            .validator,
        None
    );
}

#[test]
fn entry_eviction_never_discards_a_loading_tree_subject() {
    let mut client = client(1, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    observe_object(&mut client, &session, "/", object("", None, b"root"));
    let tree = client.refresh_observed(&session, "/", 1).unwrap();
    assert_eq!(
        client
            .refresh_observed(&session, "/other", 0)
            .unwrap_err()
            .kind,
        ClientErrorKind::Capacity
    );
    assert!(matches!(
        client.tree(&session, "/").unwrap().state,
        ObservationState::Loading(id) if id == tree.id
    ));
    client
        .complete(
            tree.id,
            encode_test_observation_response(
                &ObserveRequest {
                    path: "/".into(),
                    depth: 1,
                },
                &TestObservationResponse {
                    root: TestObservation {
                        object: object("", None, b"tree"),
                        children: vec![],
                    },
                },
                wire_limits(),
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        client.tree(&session, "/").unwrap().state,
        ObservationState::Fresh
    );
}

#[test]
fn failed_entry_and_tree_observation_metadata_is_bounded() {
    let mut client = client(2, 8, 2, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    for path in ["/one", "/two", "/three"] {
        let request = client.refresh_observed(&session, path, 0).unwrap();
        assert_eq!(
            client
                .fail_transport(request.id, "retrieval failed")
                .unwrap_err()
                .kind,
            ClientErrorKind::Transport
        );
    }
    assert!(client.object(&session, "/one").is_none());
    assert!(client.object(&session, "/two").is_some());
    assert!(client.object(&session, "/three").is_some());

    for path in ["/alpha", "/beta", "/gamma"] {
        let request = client.refresh_observed(&session, path, 1).unwrap();
        assert_eq!(
            client
                .fail_transport(request.id, "tree failed")
                .unwrap_err()
                .kind,
            ClientErrorKind::Transport
        );
    }
    assert!(client.tree(&session, "/alpha").is_none());
    assert!(client.tree(&session, "/gamma").is_some());
}

#[test]
fn in_flight_and_history_are_bounded() {
    let mut client = client(8, 8, 1, 1);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    let first = client.refresh_observed(&session, "/one", 0).unwrap();
    let error = client.refresh_observed(&session, "/two", 0).unwrap_err();
    assert_eq!(error.kind, ClientErrorKind::Capacity);
    client
        .complete(
            first.id,
            observation_response("/one", &object("one", None, b"1")),
        )
        .unwrap();

    // Diagnostic history evicts oldest entries at the configured bound.
    let stale_old = client.refresh_observed(&session, "/one", 0).unwrap();
    let stale_new = client.refresh_observed(&session, "/one", 0).unwrap_err();
    assert_eq!(stale_new.kind, ClientErrorKind::Capacity);
    client
        .complete(
            stale_old.id,
            Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(b"not json".to_vec())
                .unwrap(),
        )
        .unwrap_err();
    assert_eq!(client.diagnostics(&session).unwrap().len(), 1);
}

#[test]
fn structured_interface_keys_membership_and_scope_ref_are_preserved() {
    let mut client = client(8, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    let root = root_reference("same#::操作");
    let local = wip_protocol::InterfaceReference {
        scope: "/item".into(),
        name: root.name.clone(),
    };
    let mut value = object("item", None, b"object-v1");
    value.interfaces = vec![local.clone()];
    observe_object(&mut client, &session, "/item", value);
    for (reference, scope_ref) in [
        (root.clone(), None),
        (local.clone(), Some("scope-identity".into())),
    ] {
        observe_interface(
            &mut client,
            &session,
            FetchInterfaceResponse {
                interface: reference,
                descriptor: descriptor(TypeExpr::Integer),
                scope_ref,
                validator: None,
            },
        );
    }
    assert!(client.interface(&session, &root).is_some());
    assert_eq!(
        client
            .interface(&session, &local)
            .unwrap()
            .scope_ref
            .as_deref(),
        Some("scope-identity")
    );
    assert_eq!(
        client
            .prepare_call(&session, "/item", &root, "calculate", args(1))
            .unwrap_err()
            .kind,
        ClientErrorKind::InvalidRequest
    );
    let call = client
        .prepare_call(&session, "/item", &local, "calculate", args(1))
        .unwrap();
    let context = client.pending_call_context(call.id).unwrap();
    assert_eq!(context.request().interface.reference, local);
    assert_eq!(
        context.request().interface.scope_ref.as_deref(),
        Some("scope-identity")
    );
    assert_eq!(
        context.interface().scope_ref.as_deref(),
        Some("scope-identity")
    );
    assert_eq!(client.interface(&session, &root).unwrap().scope_ref, None);
}

fn scoped_reference(scope: &str) -> wip_protocol::InterfaceReference {
    wip_protocol::InterfaceReference {
        scope: scope.into(),
        name: "math".into(),
    }
}

fn scoped_descriptor(
    scope: &str,
    scope_ref: Option<&str>,
    validator: Option<&[u8]>,
    returns: TypeExpr,
) -> FetchInterfaceResponse {
    FetchInterfaceResponse {
        interface: scoped_reference(scope),
        scope_ref: scope_ref.map(str::to_owned),
        validator: validator.map(<[u8]>::to_vec),
        descriptor: descriptor(returns),
    }
}

fn interface_response(value: &FetchInterfaceResponse) -> Response<Vec<u8>> {
    encode_fetch_interface_response(
        &FetchInterfaceRequest {
            interface: value.interface.clone(),
        },
        value,
        wire_limits(),
    )
    .unwrap()
}

fn failure_response(
    interaction: ProtocolInteraction,
    code: ProtocolErrorCode,
) -> Response<Vec<u8>> {
    encode_protocol_error_response(
        interaction,
        &ProtocolError {
            code,
            message: "host failure".into(),
        },
        wire_limits(),
    )
    .unwrap()
}

fn scope_client() -> (Client, wip_client::SessionId) {
    let mut client = client(8, 8, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    let mut target = object("item", Some("target"), b"target-v1");
    target.interfaces = vec![scoped_reference("/scope")];
    observe_object(&mut client, &session, "/scope/item", target);
    (client, session)
}

#[test]
fn scope_binding_updates_retire_descriptors_without_reattaching_new_refs() {
    for validator in [None, Some(b"same-validator".as_slice())] {
        for new_ref in [None, Some("replacement")] {
            let (mut client, session) = scope_client();
            let old = scoped_descriptor("/scope", Some("original"), validator, TypeExpr::Integer);
            observe_interface(&mut client, &session, old.clone());
            // A same-ref ordinary state update does not end the Interface lifetime.
            observe_object(
                &mut client,
                &session,
                "/scope",
                object("scope", Some("original"), b"v1"),
            );
            observe_object(
                &mut client,
                &session,
                "/scope",
                object("scope", Some("original"), b"v2"),
            );
            assert_eq!(
                client.interface(&session, &old.interface).unwrap().state,
                ObservationState::Fresh
            );
            let call = client
                .prepare_call(
                    &session,
                    "/scope/item",
                    &old.interface,
                    "calculate",
                    args(1),
                )
                .unwrap();
            let context = client.pending_call_context(call.id).unwrap().clone();
            observe_object(
                &mut client,
                &session,
                "/scope",
                object("scope", new_ref, b"v3"),
            );
            let cached = client.interface(&session, &old.interface).unwrap();
            assert_eq!(cached.state, ObservationState::Stale);
            assert!(cached.descriptor.is_none());
            assert_eq!(cached.scope_ref.as_deref(), Some("original"));
            assert_eq!(
                client
                    .prepare_call(
                        &session,
                        "/scope/item",
                        &old.interface,
                        "calculate",
                        args(2)
                    )
                    .unwrap_err()
                    .kind,
                ClientErrorKind::MissingObservation
            );
            // Equal validators across lifetimes do not constrain the new Descriptor.
            let current = scoped_descriptor("/scope", new_ref, validator, TypeExpr::String);
            observe_interface(&mut client, &session, current.clone());
            let new_call = client
                .prepare_call(
                    &session,
                    "/scope/item",
                    &old.interface,
                    "calculate",
                    args(3),
                )
                .unwrap();
            assert_eq!(
                client
                    .pending_call_context(new_call.id)
                    .unwrap()
                    .request()
                    .interface
                    .scope_ref,
                new_ref.map(str::to_owned)
            );
            assert_eq!(context.interface().scope_ref.as_deref(), Some("original"));
            assert_eq!(context.descriptor(), &old.descriptor);
            // An in-flight call keeps the old Integer result contract after retirement.
            let result = encode_call_operation_response(
                context.request(),
                context.descriptor(),
                &CallOperationResponse {
                    result: Value::Integer(7),
                    validator: None,
                },
                wire_limits(),
            )
            .unwrap();
            let Completion::Call(record) = client.complete(call.id, result).unwrap() else {
                panic!("call completion expected")
            };
            assert!(matches!(record.outcome, CallOutcome::Success(_)));
            assert_eq!(record.context.descriptor(), &old.descriptor);
        }
    }
}

#[test]
fn fetch_scope_ref_changes_start_new_lifetimes_but_same_ref_preserves_validation() {
    for validator in [None, Some(b"reused".as_slice())] {
        let (mut client, session) = scope_client();
        let old = scoped_descriptor("/scope", Some("old"), validator, TypeExpr::Integer);
        observe_interface(&mut client, &session, old.clone());
        let contradictory = scoped_descriptor("/scope", Some("old"), validator, TypeExpr::String);
        let fetch = client
            .prepare_interface(&session, old.interface.clone())
            .unwrap();
        assert_eq!(
            client
                .complete(fetch.id, interface_response(&contradictory))
                .unwrap_err()
                .kind,
            ClientErrorKind::InvalidResponse
        );
        for scope_ref in [Some("new"), None, Some("another")] {
            let current = scoped_descriptor("/scope", scope_ref, validator, TypeExpr::String);
            observe_interface(&mut client, &session, current);
        }
    }
}

#[test]
fn missing_refs_do_not_invent_identity_or_detect_unobservable_replacement() {
    let (mut client, session) = scope_client();
    let value = scoped_descriptor("/scope", None, None, TypeExpr::Integer);
    observe_interface(&mut client, &session, value.clone());
    for validator in [b"first".as_slice(), b"replaced".as_slice()] {
        observe_object(
            &mut client,
            &session,
            "/scope",
            object("scope", None, validator),
        );
        assert_eq!(
            client.interface(&session, &value.interface).unwrap().state,
            ObservationState::Fresh
        );
    }
    let call = client
        .prepare_call(
            &session,
            "/scope/item",
            &value.interface,
            "calculate",
            args(1),
        )
        .unwrap();
    assert_eq!(
        client
            .pending_call_context(call.id)
            .unwrap()
            .request()
            .interface
            .scope_ref,
        None
    );
    let fetch = client
        .prepare_interface(&session, value.interface.clone())
        .unwrap();
    let changed = scoped_descriptor("/scope", None, None, TypeExpr::String);
    assert_eq!(
        client
            .complete(fetch.id, interface_response(&changed))
            .unwrap_err()
            .kind,
        ClientErrorKind::InvalidResponse
    );
}

#[test]
fn confirmed_scope_deletion_discards_cache_and_rejects_old_fetch_and_descendant_observe() {
    let (mut client, session) = scope_client();
    let old = scoped_descriptor("/scope", Some("old"), None, TypeExpr::Integer);
    observe_interface(&mut client, &session, old.clone());
    let old_fetch = client
        .prepare_interface(&session, old.interface.clone())
        .unwrap();
    let old_child = client.refresh_object(&session, "/scope/item").unwrap();
    let deletion = client.refresh_object(&session, "/scope").unwrap();
    client
        .complete(
            deletion.id,
            failure_response(ProtocolInteraction::Observe, ProtocolErrorCode::NotFound),
        )
        .unwrap();
    assert!(client.interface(&session, &old.interface).is_none());
    assert_eq!(
        client.object(&session, "/scope/item").unwrap().state,
        ObservationState::Stale
    );
    assert_eq!(
        client
            .complete(old_fetch.id, interface_response(&old))
            .unwrap(),
        Completion::StaleResponseRejected(old_fetch.id)
    );
    let mut target = object("item", Some("target"), b"target-v1");
    target.interfaces = vec![old.interface.clone()];
    assert_eq!(
        client
            .complete(old_child.id, observation_response("/scope/item", &target))
            .unwrap(),
        Completion::StaleResponseRejected(old_child.id)
    );
    // Republishing even without refs is observable after direct NotFound.
    observe_object(
        &mut client,
        &session,
        "/scope",
        object("scope", None, b"new"),
    );
    let new = scoped_descriptor("/scope", None, None, TypeExpr::String);
    let fetch = client
        .ensure_interface(&session, old.interface.clone())
        .unwrap()
        .unwrap();
    client.complete(fetch.id, interface_response(&new)).unwrap();
    assert_eq!(
        client
            .interface(&session, &old.interface)
            .unwrap()
            .descriptor
            .as_ref(),
        Some(&new.descriptor)
    );
    assert_eq!(client.in_flight_len(), 0);
}

#[test]
fn scope_replacement_supersedes_pending_fetch_including_first_fetch() {
    for already_cached in [false, true] {
        let (mut client, session) = scope_client();
        let old = scoped_descriptor("/scope", Some("old"), None, TypeExpr::Integer);
        if already_cached {
            observe_interface(&mut client, &session, old.clone());
        }
        let pending = client
            .prepare_interface(&session, old.interface.clone())
            .unwrap();
        observe_object(
            &mut client,
            &session,
            "/scope",
            object("scope", Some("new"), b"new"),
        );
        assert_eq!(
            client
                .complete(pending.id, interface_response(&old))
                .unwrap(),
            Completion::StaleResponseRejected(pending.id)
        );
        assert_eq!(
            client.interface(&session, &old.interface).unwrap().state,
            ObservationState::Stale
        );
        observe_interface(
            &mut client,
            &session,
            scoped_descriptor("/scope", Some("new"), None, TypeExpr::String),
        );
    }
}

#[test]
fn interface_mismatch_stales_both_observations_but_not_a_newer_fetch() {
    for newer_fetch in [false, true] {
        let (mut client, session) = scope_client();
        let old = scoped_descriptor("/scope", Some("old"), None, TypeExpr::Integer);
        observe_interface(&mut client, &session, old.clone());
        let call = client
            .prepare_call(
                &session,
                "/scope/item",
                &old.interface,
                "calculate",
                args(1),
            )
            .unwrap();
        let newer = newer_fetch.then(|| {
            client
                .prepare_interface(&session, old.interface.clone())
                .unwrap()
        });
        client
            .complete(
                call.id,
                failure_response(
                    ProtocolInteraction::CallOperation,
                    ProtocolErrorCode::InterfaceMismatch,
                ),
            )
            .unwrap();
        assert_eq!(
            client.object(&session, "/scope/item").unwrap().state,
            ObservationState::Stale
        );
        let current = client.interface(&session, &old.interface).unwrap();
        if let Some(newer) = newer {
            assert_eq!(current.state, ObservationState::Loading(newer.id));
            client.complete(newer.id, interface_response(&old)).unwrap();
        } else {
            assert_eq!(current.state, ObservationState::Stale);
            assert_eq!(current.descriptor.as_ref(), Some(&old.descriptor));
            // No automatic retry, and same-binding consistency still applies.
            assert_eq!(client.in_flight_len(), 0);
            let contradictory = scoped_descriptor("/scope", Some("old"), None, TypeExpr::String);
            let fetch = client
                .prepare_interface(&session, old.interface.clone())
                .unwrap();
            assert_eq!(
                client
                    .complete(fetch.id, interface_response(&contradictory))
                    .unwrap_err()
                    .kind,
                ClientErrorKind::InvalidResponse
            );
            // A genuinely new binding can replace an immutable old Descriptor.
            observe_interface(
                &mut client,
                &session,
                scoped_descriptor("/scope", Some("new"), None, TypeExpr::String),
            );
        }
        assert_eq!(client.call_history(&session).unwrap().len(), 1);
    }
}

#[test]
fn call_not_found_stales_target_but_only_direct_observe_confirms_scope_deletion() {
    let (mut client, session) = scope_client();
    let value = scoped_descriptor("/scope", Some("scope"), None, TypeExpr::Integer);
    observe_interface(&mut client, &session, value.clone());
    let call = client
        .prepare_call(
            &session,
            "/scope/item",
            &value.interface,
            "calculate",
            args(1),
        )
        .unwrap();
    client
        .complete(
            call.id,
            failure_response(
                ProtocolInteraction::CallOperation,
                ProtocolErrorCode::NotFound,
            ),
        )
        .unwrap();
    assert_eq!(
        client.object(&session, "/scope/item").unwrap().state,
        ObservationState::Stale
    );
    assert_eq!(
        client.interface(&session, &value.interface).unwrap().state,
        ObservationState::Fresh
    );
    assert_eq!(client.in_flight_len(), 0);
}

#[test]
fn scope_movement_does_not_rekey_cache_and_root_deletion_is_segment_aware() {
    let (mut client, session) = scope_client();
    let old = scoped_descriptor("/scope", Some("scope-id"), None, TypeExpr::Integer);
    observe_interface(&mut client, &session, old.clone());
    observe_interface(
        &mut client,
        &session,
        scoped_descriptor("/scope-other", None, None, TypeExpr::Integer),
    );
    observe_object(
        &mut client,
        &session,
        "/moved",
        object("moved", Some("scope-id"), b"moved"),
    );
    assert!(
        client
            .interface(&session, &scoped_reference("/moved"))
            .is_none()
    );
    let deletion = client.refresh_object(&session, "/scope").unwrap();
    client
        .complete(
            deletion.id,
            failure_response(ProtocolInteraction::Observe, ProtocolErrorCode::NotFound),
        )
        .unwrap();
    assert!(client.interface(&session, &old.interface).is_none());
    assert!(
        client
            .interface(&session, &scoped_reference("/scope-other"))
            .is_some()
    );
    let root = client.refresh_object(&session, "/").unwrap();
    client
        .complete(
            root.id,
            failure_response(ProtocolInteraction::Observe, ProtocolErrorCode::NotFound),
        )
        .unwrap();
    assert!(
        client
            .interface(&session, &scoped_reference("/scope-other"))
            .is_none()
    );
}

#[test]
fn edge_omission_and_local_scope_eviction_do_not_end_interface_lifetimes() {
    let mut client = client(2, 2, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    let value = scoped_descriptor("/scope", Some("scope-id"), None, TypeExpr::Integer);
    observe_interface(&mut client, &session, value.clone());
    let request = ObserveRequest {
        path: "/".into(),
        depth: 1,
    };
    for children in [
        vec![TestObservation {
            object: object("scope", Some("scope-id"), b"scope"),
            children: vec![],
        }],
        vec![],
    ] {
        let pending = client.refresh_children(&session, "/").unwrap();
        client
            .complete(
                pending.id,
                encode_test_observation_response(
                    &request,
                    &TestObservationResponse {
                        root: TestObservation {
                            object: object("", None, b"root"),
                            children,
                        },
                    },
                    wire_limits(),
                )
                .unwrap(),
            )
            .unwrap();
    }
    assert_eq!(
        client.object(&session, "/scope").unwrap().state,
        ObservationState::Stale
    );
    assert_eq!(
        client.interface(&session, &value.interface).unwrap().state,
        ObservationState::Fresh
    );
    for path in ["/zzz", "/zzzz"] {
        observe_object(
            &mut client,
            &session,
            path,
            object(path.trim_start_matches('/'), None, b"other"),
        );
    }
    assert!(client.object(&session, "/scope").is_none());
    assert_eq!(
        client.interface(&session, &value.interface).unwrap().state,
        ObservationState::Fresh
    );
    // Eviction was not evidence of deletion; consistency still holds.
    let pending = client
        .prepare_interface(&session, value.interface.clone())
        .unwrap();
    let changed = scoped_descriptor("/scope", Some("scope-id"), None, TypeExpr::String);
    assert_eq!(
        client
            .complete(pending.id, interface_response(&changed))
            .unwrap_err()
            .kind,
        ClientErrorKind::InvalidResponse
    );
}

#[test]
fn scope_evidence_can_retire_a_retained_descriptor_while_a_later_refresh_loads() {
    for deleted in [false, true] {
        let (mut client, session) = scope_client();
        let old = scoped_descriptor("/scope", Some("old"), None, TypeExpr::Integer);
        observe_interface(&mut client, &session, old.clone());
        let scope_request = client.refresh_object(&session, "/scope").unwrap();
        let later_fetch = client
            .prepare_interface(&session, old.interface.clone())
            .unwrap();
        let response = if deleted {
            failure_response(ProtocolInteraction::Observe, ProtocolErrorCode::NotFound)
        } else {
            observation_response("/scope", &object("scope", Some("new"), b"new"))
        };
        client.complete(scope_request.id, response).unwrap();
        assert_eq!(
            client
                .complete(later_fetch.id, interface_response(&old))
                .unwrap(),
            Completion::StaleResponseRejected(later_fetch.id)
        );
        assert!(
            client
                .interface(&session, &old.interface)
                .is_none_or(|cached| cached.state == ObservationState::Stale)
        );
    }
}

#[test]
fn deletion_keeps_in_flight_result_contract_and_isolated_session_cache() {
    let (mut client, session) = scope_client();
    let other = client
        .open_session(
            "https://example.test/wip",
            SecurityContext::new("other-tenant"),
        )
        .unwrap();
    let old = scoped_descriptor("/scope", Some("old"), None, TypeExpr::Integer);
    for id in [&session, &other] {
        observe_interface(&mut client, id, old.clone());
    }
    let call = client
        .prepare_call(
            &session,
            "/scope/item",
            &old.interface,
            "calculate",
            args(1),
        )
        .unwrap();
    let context = client.pending_call_context(call.id).unwrap().clone();
    let deletion = client.refresh_object(&session, "/scope").unwrap();
    client
        .complete(
            deletion.id,
            failure_response(ProtocolInteraction::Observe, ProtocolErrorCode::NotFound),
        )
        .unwrap();
    assert!(client.interface(&session, &old.interface).is_none());
    assert_eq!(
        client.interface(&other, &old.interface).unwrap().state,
        ObservationState::Fresh
    );
    let response = encode_call_operation_response(
        context.request(),
        context.descriptor(),
        &CallOperationResponse {
            result: Value::Integer(42),
            validator: None,
        },
        wire_limits(),
    )
    .unwrap();
    let Completion::Call(record) = client.complete(call.id, response).unwrap() else {
        panic!("call expected")
    };
    assert!(matches!(record.outcome, CallOutcome::Success(_)));
    assert_eq!(record.context.descriptor(), &old.descriptor);
    assert!(client.interface(&session, &old.interface).is_none());
}

#[test]
fn object_response_scope_placement_is_validated_without_fetching_ancestors() {
    for scope in ["/sibling", "/item/child", "/ite"] {
        let (mut client, session) = scope_client();
        let request = client.refresh_object(&session, "/item").unwrap();
        let json = format!(
            r#"{{"object":{{"name":"item","interfaces":[{{"scope":"{scope}","name":"math"}}]}}}}"#
        );
        let response = Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(json.into_bytes())
            .unwrap();
        assert_eq!(
            client.complete(request.id, response).unwrap_err().kind,
            ClientErrorKind::InvalidResponse
        );
        assert_eq!(client.in_flight_len(), 0);
    }
}

#[test]
fn direct_deletion_is_lifetime_evidence_even_without_any_refs() {
    let (mut client, session) = scope_client();
    let old = scoped_descriptor("/scope", None, None, TypeExpr::Integer);
    observe_interface(&mut client, &session, old.clone());
    let deletion = client.refresh_object(&session, "/scope").unwrap();
    client
        .complete(
            deletion.id,
            failure_response(ProtocolInteraction::Observe, ProtocolErrorCode::NotFound),
        )
        .unwrap();
    assert!(client.interface(&session, &old.interface).is_none());
    let current = scoped_descriptor("/scope", None, None, TypeExpr::String);
    observe_interface(&mut client, &session, current.clone());
    assert_eq!(
        client
            .interface(&session, &old.interface)
            .unwrap()
            .descriptor
            .as_ref(),
        Some(&current.descriptor)
    );
}

#[test]
fn old_scope_evidence_does_not_retire_a_newer_successful_fetch() {
    let (mut client, session) = scope_client();
    let old_scope = client.refresh_object(&session, "/scope").unwrap();
    let current = scoped_descriptor("/scope", Some("new"), None, TypeExpr::String);
    observe_interface(&mut client, &session, current.clone());
    client
        .complete(
            old_scope.id,
            observation_response("/scope", &object("scope", Some("old"), b"old")),
        )
        .unwrap();
    assert_eq!(
        client
            .interface(&session, &current.interface)
            .unwrap()
            .state,
        ObservationState::Fresh
    );
    assert_eq!(
        client
            .interface(&session, &current.interface)
            .unwrap()
            .scope_ref
            .as_deref(),
        Some("new")
    );
}

#[test]
fn sibling_interface_scope_evidence_retires_old_names_and_fetch_ownership() {
    for new_ref in [None, Some("replacement")] {
        for loading in [false, true] {
            let (mut client, session) = scope_client();
            let old = scoped_descriptor("/scope", Some("old"), None, TypeExpr::Integer);
            let mut sibling = old.clone();
            sibling.interface.name = "other".into();
            observe_interface(&mut client, &session, old.clone());
            observe_interface(&mut client, &session, sibling.clone());
            let call = client
                .prepare_call(
                    &session,
                    "/scope/item",
                    &old.interface,
                    "calculate",
                    args(1),
                )
                .unwrap();
            let context = client.pending_call_context(call.id).unwrap().clone();
            let pending = loading.then(|| {
                client
                    .prepare_interface(&session, old.interface.clone())
                    .unwrap()
            });
            // Same-ref updates through a sibling preserve the old lifetime.
            observe_interface(&mut client, &session, sibling.clone());
            assert!(
                client
                    .interface(&session, &old.interface)
                    .unwrap()
                    .descriptor
                    .is_some()
            );
            sibling.scope_ref = new_ref.map(str::to_owned);
            sibling.descriptor = descriptor(TypeExpr::String);
            observe_interface(&mut client, &session, sibling.clone());
            let cached = client.interface(&session, &old.interface).unwrap();
            assert_eq!(cached.state, ObservationState::Stale);
            assert!(cached.descriptor.is_none());
            assert_eq!(cached.scope_ref.as_deref(), Some("old"));
            assert_eq!(
                client
                    .prepare_call(
                        &session,
                        "/scope/item",
                        &old.interface,
                        "calculate",
                        args(2)
                    )
                    .unwrap_err()
                    .kind,
                ClientErrorKind::MissingObservation
            );
            if let Some(pending) = pending {
                assert_eq!(
                    client
                        .complete(pending.id, interface_response(&old))
                        .unwrap(),
                    Completion::StaleResponseRejected(pending.id)
                );
            }
            let current = scoped_descriptor("/scope", new_ref, None, TypeExpr::String);
            observe_interface(&mut client, &session, current.clone());
            assert_eq!(
                client
                    .interface(&session, &old.interface)
                    .unwrap()
                    .descriptor
                    .as_ref(),
                Some(&current.descriptor)
            );
            // Retirement of the shared scope does not mutate prepared result contracts.
            let result = encode_call_operation_response(
                context.request(),
                context.descriptor(),
                &CallOperationResponse {
                    result: Value::Integer(7),
                    validator: None,
                },
                wire_limits(),
            )
            .unwrap();
            let Completion::Call(record) = client.complete(call.id, result).unwrap() else {
                panic!("call expected")
            };
            assert!(matches!(record.outcome, CallOutcome::Success(_)));
            assert_eq!(record.context.descriptor(), &old.descriptor);
        }
    }
}

#[test]
fn cross_name_scope_evidence_rejects_older_first_fetch_but_preserves_newer_observations() {
    let (mut client, session) = scope_client();
    let old = scoped_descriptor("/scope", Some("old"), None, TypeExpr::Integer);
    let first_fetch = client
        .prepare_interface(&session, old.interface.clone())
        .unwrap();
    let mut sibling = scoped_descriptor("/scope", Some("new"), None, TypeExpr::String);
    sibling.interface.name = "other".into();
    observe_interface(&mut client, &session, sibling.clone());
    assert_eq!(
        client
            .complete(first_fetch.id, interface_response(&old))
            .unwrap(),
        Completion::StaleResponseRejected(first_fetch.id)
    );
    // A delayed older response cannot retire a newer accepted sibling.
    let old_request = client
        .prepare_interface(&session, old.interface.clone())
        .unwrap();
    sibling.scope_ref = Some("newer".into());
    observe_interface(&mut client, &session, sibling.clone());
    assert_eq!(
        client
            .complete(old_request.id, interface_response(&old))
            .unwrap(),
        Completion::StaleResponseRejected(old_request.id)
    );
    assert_eq!(
        client
            .interface(&session, &sibling.interface)
            .unwrap()
            .state,
        ObservationState::Fresh
    );
    assert_eq!(
        client
            .interface(&session, &sibling.interface)
            .unwrap()
            .scope_ref,
        sibling.scope_ref
    );
}

#[test]
fn observed_object_ref_changes_retire_descriptors_with_omitted_scope_ref() {
    for new_ref in [None, Some("replacement")] {
        for validator in [None, Some(b"same-validator".as_slice())] {
            for object_first in [false, true] {
                let (mut client, session) = scope_client();
                let old = scoped_descriptor("/scope", None, validator, TypeExpr::Integer);
                if object_first {
                    observe_object(
                        &mut client,
                        &session,
                        "/scope",
                        object("scope", Some("old"), b"v1"),
                    );
                }
                observe_interface(&mut client, &session, old.clone());
                if !object_first {
                    observe_object(
                        &mut client,
                        &session,
                        "/scope",
                        object("scope", Some("old"), b"v1"),
                    );
                }
                observe_object(
                    &mut client,
                    &session,
                    "/scope",
                    object("scope", Some("old"), b"v2"),
                );
                assert_eq!(
                    client.interface(&session, &old.interface).unwrap().state,
                    ObservationState::Fresh
                );
                let call = client
                    .prepare_call(
                        &session,
                        "/scope/item",
                        &old.interface,
                        "calculate",
                        args(1),
                    )
                    .unwrap();
                assert_eq!(
                    client
                        .pending_call_context(call.id)
                        .unwrap()
                        .request()
                        .interface
                        .scope_ref,
                    None
                );
                let pending = client
                    .prepare_interface(&session, old.interface.clone())
                    .unwrap();
                observe_object(
                    &mut client,
                    &session,
                    "/scope",
                    object("scope", new_ref, b"v3"),
                );
                let cached = client.interface(&session, &old.interface).unwrap();
                assert_eq!(cached.state, ObservationState::Stale);
                assert!(cached.descriptor.is_none());
                assert_eq!(cached.scope_ref, None);
                assert_eq!(
                    client
                        .complete(pending.id, interface_response(&old))
                        .unwrap(),
                    Completion::StaleResponseRejected(pending.id)
                );
                assert_eq!(
                    client
                        .prepare_call(
                            &session,
                            "/scope/item",
                            &old.interface,
                            "calculate",
                            args(2)
                        )
                        .unwrap_err()
                        .kind,
                    ClientErrorKind::MissingObservation
                );
                // A valid republished ref-less Descriptor no longer conflicts with the old lifetime.
                let current = scoped_descriptor("/scope", None, validator, TypeExpr::String);
                observe_interface(&mut client, &session, current.clone());
                let new_call = client
                    .prepare_call(
                        &session,
                        "/scope/item",
                        &old.interface,
                        "calculate",
                        args(3),
                    )
                    .unwrap();
                assert_eq!(
                    client
                        .pending_call_context(new_call.id)
                        .unwrap()
                        .request()
                        .interface
                        .scope_ref,
                    None
                );
                assert_eq!(
                    client
                        .pending_call_context(new_call.id)
                        .unwrap()
                        .descriptor(),
                    &current.descriptor
                );
            }
        }
    }
}

#[test]
fn omitted_scope_ref_keeps_same_lifetime_validation_and_private_evidence_survives_eviction() {
    let mut client = client(2, 4, 8, 8);
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    observe_object(
        &mut client,
        &session,
        "/scope",
        object("scope", Some("old"), b"v1"),
    );
    let old = scoped_descriptor("/scope", None, None, TypeExpr::Integer);
    observe_interface(&mut client, &session, old.clone());
    observe_object(
        &mut client,
        &session,
        "/scope",
        object("scope", Some("old"), b"v2"),
    );
    let pending = client
        .prepare_interface(&session, old.interface.clone())
        .unwrap();
    let changed = scoped_descriptor("/scope", None, None, TypeExpr::String);
    assert_eq!(
        client
            .complete(pending.id, interface_response(&changed))
            .unwrap_err()
            .kind,
        ClientErrorKind::InvalidResponse
    );
    // Restore freshness without changing the same-lifetime descriptor.
    observe_interface(&mut client, &session, old.clone());
    for path in ["/zzz", "/zzzz"] {
        observe_object(
            &mut client,
            &session,
            path,
            object(path.trim_start_matches('/'), None, b"other"),
        );
    }
    assert!(client.object(&session, "/scope").is_none());
    assert_eq!(
        client.interface(&session, &old.interface).unwrap().state,
        ObservationState::Fresh
    );
    observe_object(
        &mut client,
        &session,
        "/scope",
        object("scope", Some("new"), b"v3"),
    );
    assert_eq!(
        client.interface(&session, &old.interface).unwrap().state,
        ObservationState::Stale
    );
    observe_interface(&mut client, &session, changed.clone());
    assert_eq!(
        client
            .interface(&session, &old.interface)
            .unwrap()
            .descriptor
            .as_ref(),
        Some(&changed.descriptor)
    );
    assert_eq!(
        client
            .interface(&session, &old.interface)
            .unwrap()
            .scope_ref,
        None
    );
}

#[test]
fn sibling_evidence_tracks_ref_less_descriptors_without_rewriting_call_metadata() {
    let (mut client, session) = scope_client();
    let old = scoped_descriptor("/scope", None, None, TypeExpr::Integer);
    observe_interface(&mut client, &session, old.clone());
    let mut sibling = scoped_descriptor("/scope", Some("old"), None, TypeExpr::Integer);
    sibling.interface.name = "other".into();
    observe_interface(&mut client, &session, sibling.clone());
    let call = client
        .prepare_call(
            &session,
            "/scope/item",
            &old.interface,
            "calculate",
            args(1),
        )
        .unwrap();
    assert_eq!(
        client
            .pending_call_context(call.id)
            .unwrap()
            .request()
            .interface
            .scope_ref,
        None
    );
    sibling.scope_ref = Some("new".into());
    observe_interface(&mut client, &session, sibling);
    assert_eq!(
        client.interface(&session, &old.interface).unwrap().state,
        ObservationState::Stale
    );
    assert_eq!(
        client
            .interface(&session, &old.interface)
            .unwrap()
            .scope_ref,
        None
    );
}

#[test]
fn corroborated_same_scope_ref_preserves_immutability_when_wire_metadata_becomes_present() {
    let (mut client, session) = scope_client();
    observe_object(
        &mut client,
        &session,
        "/scope",
        object("scope", Some("same"), b"v1"),
    );
    let old = scoped_descriptor("/scope", None, None, TypeExpr::Integer);
    observe_interface(&mut client, &session, old.clone());
    let pending = client
        .prepare_interface(&session, old.interface.clone())
        .unwrap();
    let changed = scoped_descriptor("/scope", Some("same"), None, TypeExpr::String);
    assert_eq!(
        client
            .complete(pending.id, interface_response(&changed))
            .unwrap_err()
            .kind,
        ClientErrorKind::InvalidResponse
    );
    let unchanged = scoped_descriptor("/scope", Some("same"), None, TypeExpr::Integer);
    observe_interface(&mut client, &session, unchanged);
}

#[test]
fn retained_object_binding_survives_loading_and_failed_refresh_for_first_ref_less_fetch() {
    for new_ref in [None, Some("new")] {
        for accepted_descriptor in [false, true] {
            for failed_refresh in [false, true] {
                let (mut client, session) = scope_client();
                observe_object(
                    &mut client,
                    &session,
                    "/scope",
                    object("scope", Some("old"), b"v1"),
                );
                let loading_scope = client.refresh_object(&session, "/scope").unwrap();
                if failed_refresh {
                    client
                        .fail_transport(loading_scope.id, "temporary failure")
                        .unwrap_err();
                }
                let old = scoped_descriptor("/scope", None, None, TypeExpr::Integer);
                let pending = client
                    .prepare_interface(&session, old.interface.clone())
                    .unwrap();
                if accepted_descriptor {
                    client
                        .complete(pending.id, interface_response(&old))
                        .unwrap();
                }
                observe_object(
                    &mut client,
                    &session,
                    "/scope",
                    object("scope", new_ref, b"v2"),
                );
                let cached = client.interface(&session, &old.interface).unwrap();
                assert_eq!(cached.state, ObservationState::Stale);
                assert!(cached.descriptor.is_none());
                assert_eq!(cached.scope_ref, None);
                if !accepted_descriptor {
                    assert_eq!(
                        client
                            .complete(pending.id, interface_response(&old))
                            .unwrap(),
                        Completion::StaleResponseRejected(pending.id)
                    );
                }
                assert_eq!(
                    client
                        .prepare_call(
                            &session,
                            "/scope/item",
                            &old.interface,
                            "calculate",
                            args(1)
                        )
                        .unwrap_err()
                        .kind,
                    ClientErrorKind::MissingObservation
                );
                let current = scoped_descriptor("/scope", None, None, TypeExpr::String);
                observe_interface(&mut client, &session, current.clone());
                let call = client
                    .prepare_call(
                        &session,
                        "/scope/item",
                        &old.interface,
                        "calculate",
                        args(2),
                    )
                    .unwrap();
                assert_eq!(
                    client
                        .pending_call_context(call.id)
                        .unwrap()
                        .request()
                        .interface
                        .scope_ref,
                    None
                );
                assert_eq!(
                    client.pending_call_context(call.id).unwrap().descriptor(),
                    &current.descriptor
                );
                if !failed_refresh {
                    assert_eq!(
                        client
                            .complete(
                                loading_scope.id,
                                observation_response(
                                    "/scope",
                                    &object("scope", Some("old"), b"v1")
                                )
                            )
                            .unwrap(),
                        Completion::StaleResponseRejected(loading_scope.id)
                    );
                    assert_eq!(
                        client.interface(&session, &old.interface).unwrap().state,
                        ObservationState::Fresh
                    );
                }
            }
        }
    }
}

#[test]
fn first_ref_less_descriptor_keeps_lifetime_evidence_when_scope_is_edge_stale() {
    let (mut client, session) = scope_client();
    let request = ObserveRequest {
        path: "/".into(),
        depth: 1,
    };
    for children in [
        vec![TestObservation {
            object: object("scope", Some("old"), b"scope"),
            children: vec![],
        }],
        vec![],
    ] {
        let pending = client.refresh_children(&session, "/").unwrap();
        client
            .complete(
                pending.id,
                encode_test_observation_response(
                    &request,
                    &TestObservationResponse {
                        root: TestObservation {
                            object: object("", None, b"root"),
                            children,
                        },
                    },
                    wire_limits(),
                )
                .unwrap(),
            )
            .unwrap();
    }
    assert_eq!(
        client.object(&session, "/scope").unwrap().state,
        ObservationState::Stale
    );
    let old = scoped_descriptor("/scope", None, None, TypeExpr::Integer);
    observe_interface(&mut client, &session, old.clone());
    observe_object(
        &mut client,
        &session,
        "/scope",
        object("scope", Some("new"), b"new"),
    );
    assert_eq!(
        client.interface(&session, &old.interface).unwrap().state,
        ObservationState::Stale
    );
    assert!(
        client
            .interface(&session, &old.interface)
            .unwrap()
            .descriptor
            .is_none()
    );
}

#[test]
fn confirmed_missing_scope_does_not_corroborate_republished_descriptor_with_expired_ref() {
    let (mut client, session) = scope_client();
    observe_object(
        &mut client,
        &session,
        "/scope",
        object("scope", Some("old"), b"v1"),
    );
    let deletion = client.refresh_object(&session, "/scope").unwrap();
    client
        .complete(
            deletion.id,
            failure_response(ProtocolInteraction::Observe, ProtocolErrorCode::NotFound),
        )
        .unwrap();
    let current = scoped_descriptor("/scope", None, None, TypeExpr::String);
    observe_interface(&mut client, &session, current.clone());
    // Loading state alone must not make the confirmed ended Object binding live.
    let loading = client.refresh_object(&session, "/scope").unwrap();
    observe_interface(&mut client, &session, current.clone());
    client
        .complete(
            loading.id,
            observation_response("/scope", &object("scope", Some("new"), b"v2")),
        )
        .unwrap();
    assert_eq!(
        client
            .interface(&session, &current.interface)
            .unwrap()
            .state,
        ObservationState::Fresh
    );
    assert_eq!(
        client
            .interface(&session, &current.interface)
            .unwrap()
            .scope_ref,
        None
    );
    // A successful new observation restores tracking for subsequent replacements.
    observe_object(
        &mut client,
        &session,
        "/scope",
        object("scope", Some("another"), b"v3"),
    );
    assert_eq!(
        client
            .interface(&session, &current.interface)
            .unwrap()
            .state,
        ObservationState::Stale
    );
}
