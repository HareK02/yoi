use std::collections::BTreeMap;
use wip_protocol::{
    CallOperationRequest, FetchInterfaceRequest, FetchInterfaceResponse, InterfaceDescriptor,
    InterfaceReference, InterfaceTarget, Object, ObjectObservation, ObserveRequest,
    OperationDeclaration, ReturnDeclaration, Target, TypeExpr, ValidationErrorKind,
};

fn reference(scope: &str, name: &str) -> InterfaceReference {
    InterfaceReference {
        scope: scope.into(),
        name: name.into(),
    }
}

fn object(path: &str, interfaces: Vec<InterfaceReference>) -> Object {
    Object {
        name: path.rsplit('/').next().unwrap().into(),
        description: None,
        interfaces,
        r#ref: None,
        validator: None,
    }
}

fn descriptor() -> InterfaceDescriptor {
    InterfaceDescriptor {
        format: wip_protocol::INTERFACE_FORMAT_V1.into(),
        documentation: None,
        types: vec![],
        operations: vec![OperationDeclaration {
            name: "run".into(),
            documentation: None,
            parameters: vec![],
            returns: ReturnDeclaration {
                documentation: None,
                r#type: TypeExpr::Unit,
            },
        }],
    }
}

#[test]
fn scope_is_segment_based_ancestor_or_self_not_indexability() {
    for (scope, path) in [
        ("/", "/"),
        ("/", "/a/b"),
        ("/a", "/a"),
        ("/a", "/a/b"),
        ("/a/b", "/a/b/c"),
    ] {
        reference(scope, "read").validate_for_path(path).unwrap();
        object(path, vec![reference(scope, "read")])
            .validate_at_path(path)
            .unwrap();
    }
    for (scope, path) in [
        ("/a", "/ab"),
        ("/a", "/ab/c"),
        ("/a/b", "/a/c"),
        ("/a/b", "/a"),
        ("/a", "/"),
    ] {
        assert!(matches!(
            reference(scope, "read")
                .validate_for_path(path)
                .unwrap_err()
                .kind,
            ValidationErrorKind::InterfaceScopeMismatch { .. }
        ));
        // Without the placement context, the Object/reference shape is valid.
        let object = object(path, vec![reference(scope, "read")]);
        object.validate().unwrap();
        assert!(matches!(
            object.validate_at_path(path).unwrap_err().kind,
            ValidationErrorKind::InterfaceScopeMismatch { .. }
        ));
    }
}

#[test]
fn shape_validation_does_not_normalize_or_parse_fields() {
    for scope in ["", "relative", "//a", "/a/", "/a//b", "/a/./b", "/a/../b"] {
        let reference = reference(scope, "read");
        assert!(matches!(
            reference.validate().unwrap_err().kind,
            ValidationErrorKind::InvalidPath { .. }
        ));
        assert!(
            FetchInterfaceRequest {
                interface: reference.clone()
            }
            .validate()
            .is_err()
        );
        assert!(object("/a", vec![reference]).validate().is_err());
    }
    assert_eq!(
        reference("/", "").validate().unwrap_err().kind,
        ValidationErrorKind::EmptyInterfaceName
    );
    for name in [".", "..", "a/b", "::", "#", "読み::書き#λ", "e\u{301}"] {
        let reference = reference("/世::界/#/%2F", name);
        reference.validate().unwrap();
        assert_eq!(reference.name, name);
        assert_eq!(reference.scope, "/世::界/#/%2F");
    }
    assert_ne!(reference("/", "é"), reference("/", "e\u{301}"));
    assert_ne!(reference("/A", "read"), reference("/a", "read"));
    assert_ne!(reference("/%61", "read"), reference("/a", "read"));
}

#[test]
fn duplicate_membership_and_correlation_use_complete_pair() {
    let root = reference("/", "read");
    let scoped = reference("/a", "read");
    let mut object = object("/a", vec![root.clone(), scoped.clone()]);
    object.validate_at_path("/a").unwrap();
    assert!(object.interfaces.contains(&scoped));
    assert!(!object.interfaces.contains(&reference("/b", "read")));
    object.interfaces.push(root.clone());
    assert_eq!(
        object.validate().unwrap_err().kind,
        ValidationErrorKind::DuplicateInterface {
            reference: root.clone()
        }
    );
    let request = FetchInterfaceRequest { interface: root };
    let mut response = FetchInterfaceResponse {
        interface: scoped,
        scope_ref: None,
        descriptor: descriptor(),
        validator: None,
    };
    assert!(matches!(
        response.validate_for(&request).unwrap_err().kind,
        ValidationErrorKind::InterfaceResponseMismatch { .. }
    ));
    response.interface = request.interface.clone();
    for scope_ref in [None, Some(String::new()), Some("opaque::世界#/%".into())] {
        response.scope_ref = scope_ref;
        response.validate_for(&request).unwrap();
    }
    response.interface.name.clear();
    assert_eq!(
        response.validate_for(&request).unwrap_err().kind,
        ValidationErrorKind::EmptyInterfaceName
    );
}

#[test]
fn observe_checks_scope_at_every_node_including_depth_boundary() {
    let request = ObserveRequest {
        path: "/a".into(),
        depth: 1,
    };
    let mut response = ObjectObservation {
        object: object("/a", vec![reference("/a", "read")]),
        children: Some(vec![ObjectObservation {
            object: object("/a/b", vec![reference("/a/b", "read")]),
            children: None,
        }]),
    };
    request.validate_response(&response).unwrap();
    response.children.as_mut().unwrap()[0].object.interfaces[0].scope = "/a/c".into();
    assert!(matches!(
        request.validate_response(&response).unwrap_err().kind,
        ValidationErrorKind::InterfaceScopeMismatch { .. }
    ));
}

#[test]
fn fetch_needs_no_target_path_and_call_separates_shape_from_scope_selection() {
    let reference = reference("/some/unrelated/scope", "read");
    let fetch = FetchInterfaceRequest {
        interface: reference.clone(),
    };
    fetch.validate().unwrap();
    FetchInterfaceResponse {
        interface: reference.clone(),
        scope_ref: None,
        descriptor: descriptor(),
        validator: None,
    }
    .validate_for(&fetch)
    .unwrap();
    let mut call = CallOperationRequest {
        target: Target {
            path: "/a".into(),
            validator: None,
        },
        interface: InterfaceTarget {
            reference,
            scope_ref: Some("opaque".into()),
            validator: None,
        },
        operation: "run".into(),
        arguments: BTreeMap::new(),
    };
    call.validate().unwrap();
    call.validate_with(&descriptor()).unwrap();
    // This stage is Host InterfaceMismatch, after NotFound, not shape InvalidRequest.
    assert!(matches!(
        call.interface
            .reference
            .validate_for_path(&call.target.path)
            .unwrap_err()
            .kind,
        ValidationErrorKind::InterfaceScopeMismatch { .. }
    ));
    call.interface.reference.name.clear();
    assert_eq!(
        call.validate().unwrap_err().kind,
        ValidationErrorKind::EmptyInterfaceName
    );
    assert_eq!(
        call.validate_with(&descriptor()).unwrap_err().kind,
        ValidationErrorKind::EmptyInterfaceName
    );
}
