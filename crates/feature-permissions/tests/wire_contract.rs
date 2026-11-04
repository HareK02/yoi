//! Public JSON/TOML boundary tests using the production Serde DTOs and shared Web
//! fixtures. No resolver, Host, storage, or authentication boundary is replaced or
//! exercised here. Run with `cargo test -p feature-permissions --test wire_contract`.

use feature_permissions::{
    NamespaceCatalog, NamespaceDeclaration, NamespacePermissions, PermissionAssignment,
    PermissionCatalog, PermissionDefinition, PermissionReference, SetContribution,
    ValidatePermissionsRequest,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::fmt::Debug;

const ASSIGNMENT_JSON: &str = include_str!("fixtures/assignment.json");
const ASSIGNMENT_TOML: &str = include_str!("fixtures/assignment.toml");
const CATALOG_JSON: &str = include_str!("fixtures/catalog.json");
const CATALOG_TOML: &str = include_str!("fixtures/catalog.toml");

fn paired_fixture<T>(json: &str, toml: &str) -> T
where
    T: DeserializeOwned + Serialize + PartialEq + Debug,
{
    let from_json: T = serde_json::from_str(json).expect("valid JSON fixture");
    let from_toml: T = toml::from_str(toml).expect("valid TOML fixture");
    assert_eq!(
        from_json, from_toml,
        "text formats must have the same meaning"
    );
    assert_eq!(
        serde_json::to_value(&from_json).unwrap(),
        serde_json::from_str::<Value>(json).unwrap(),
        "serialization must not drop or synthesize wire fields"
    );
    assert_eq!(
        serde_json::from_str::<T>(&serde_json::to_string(&from_toml).unwrap()).unwrap(),
        from_json
    );
    assert_eq!(
        toml::from_str::<T>(&toml::to_string(&from_json).unwrap()).unwrap(),
        from_toml
    );
    from_json
}

fn reject_in_both_formats<T: DeserializeOwned>(rule: &str, input: Value) {
    assert!(
        serde_json::from_value::<T>(input.clone()).is_err(),
        "JSON accepted {rule}: {input}"
    );
    let text = toml::to_string(&input).expect("rejection case must be representable as TOML");
    assert!(
        toml::from_str::<T>(&text).is_err(),
        "TOML accepted {rule}: {text}"
    );
}

fn reject_authority_and_payload_fields<T: DeserializeOwned>(input: Value) {
    for (field, value) in [
        ("payload", json!({ "path": "/untrusted" })),
        ("owner", json!("other-feature")),
        ("workspace", json!("other-workspace")),
    ] {
        let mut modified = input.clone();
        modified
            .as_object_mut()
            .unwrap()
            .insert(field.into(), value);
        reject_in_both_formats::<T>(field, modified);
    }
}

#[test]
fn assignment_text_formats_preserve_discriminants_and_multisegment_keys() {
    let assignment: PermissionAssignment = paired_fixture(ASSIGNMENT_JSON, ASSIGNMENT_TOML);
    // The same spelling is intentional: the tag, not a prefix, chooses the kind.
    assert!(
        matches!(&assignment.grants[0], PermissionReference::Set(id) if id.as_str() == "ticket")
    );
    let PermissionReference::Namespace(namespace) = &assignment.grants[1] else {
        panic!("namespace reference lost its discriminant");
    };
    assert_eq!(namespace.namespace.as_str(), "ticket");
    assert_eq!(
        namespace
            .keys
            .iter()
            .map(|key| key.as_str())
            .collect::<Vec<_>>(),
        ["ticket.hoge", "ticket.hoge.fuga.finish"]
    );
}

#[test]
fn namespace_catalog_roundtrip_preserves_only_explicit_multilevel_children() {
    let catalog: PermissionCatalog = paired_fixture(CATALOG_JSON, CATALOG_TOML);
    let namespace = &catalog.namespaces[0];
    // Also roundtrip the standalone management DTO, not only its outer envelope.
    let standalone: NamespaceCatalog =
        serde_json::from_value(serde_json::to_value(namespace).unwrap()).unwrap();
    assert_eq!(&standalone, namespace);
    let permissions = &standalone.declaration.permissions;
    let edges: Vec<_> = permissions
        .iter()
        .map(|permission| {
            (
                permission.key.as_str(),
                permission
                    .children
                    .iter()
                    .map(|key| key.as_str())
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    assert_eq!(
        edges,
        [
            ("ticket.hoge", vec!["ticket.hoge.fuga"]),
            ("ticket.hoge.fuga", vec!["ticket.hoge.fuga.finish"]),
            ("ticket.hoge.fuga.finish", vec![]),
            ("ticket.hoge.unlinked", vec![]),
        ]
    );
    assert_eq!(
        serde_json::to_value(&standalone.declaration.sets[0]).unwrap(),
        json!({
            "set": "ticket",
            "children": [{"kind": "namespace", "value": {"namespace": "ticket", "keys": ["ticket.hoge"]}}]
        })
    );
}

#[test]
fn explicit_empty_and_revocation_survive_without_defaults() {
    for input in [
        json!({"grants": [], "exclusions": [], "revoked": false}),
        json!({"grants": [], "exclusions": [], "revoked": true}),
        json!({
            "grants": [{"kind": "namespace", "value": {"namespace": "ticket", "keys": []}}],
            "exclusions": [],
            "revoked": false
        }),
    ] {
        paired_fixture::<PermissionAssignment>(
            &serde_json::to_string(&input).unwrap(),
            &toml::to_string(&input).unwrap(),
        );
    }
    for missing in ["grants", "exclusions", "revoked"] {
        let mut input = json!({"grants": [], "exclusions": [], "revoked": false});
        input.as_object_mut().unwrap().remove(missing);
        reject_in_both_formats::<PermissionAssignment>(missing, input);
    }
}

#[test]
fn reference_kind_and_shape_must_match_in_json_and_toml() {
    for (rule, input) in [
        ("unknown kind", json!({"kind": "all", "value": "ticket"})),
        (
            "case-sensitive kind",
            json!({"kind": "Set", "value": "ticket"}),
        ),
        ("missing kind", json!({"value": "ticket"})),
        ("missing value", json!({"kind": "set"})),
        (
            "set with namespace shape",
            json!({"kind": "set", "value": {"namespace": "ticket", "keys": []}}),
        ),
        (
            "namespace with set shape",
            json!({"kind": "namespace", "value": "ticket"}),
        ),
        (
            "namespace missing keys",
            json!({"kind": "namespace", "value": {"namespace": "ticket"}}),
        ),
        (
            "keys are not a scalar",
            json!({"kind": "namespace", "value": {"namespace": "ticket", "keys": "ticket.hoge"}}),
        ),
        (
            "keys carry no data",
            json!({"kind": "namespace", "value": {"namespace": "ticket", "keys": [{"key": "ticket.hoge", "payload": {}}]}}),
        ),
    ] {
        reject_in_both_formats::<PermissionReference>(rule, input);
    }
}

#[test]
fn external_identifiers_reject_invalid_segments_without_truncation() {
    for invalid in ["", "Ticket", "ticket.read", "ticket read", "*", "1ticket"] {
        reject_in_both_formats::<PermissionReference>(
            "invalid set identifier",
            json!({"kind": "set", "value": invalid}),
        );
        reject_in_both_formats::<NamespacePermissions>(
            "invalid namespace identifier",
            json!({"namespace": invalid, "keys": []}),
        );
    }
    for invalid in [
        "ticket",
        "ticket..read",
        ".ticket.read",
        "ticket.read.",
        "ticket.Read",
        "ticket.*",
        "ticket.read path",
    ] {
        reject_in_both_formats::<NamespacePermissions>(
            "invalid assigned key",
            json!({"namespace": "ticket", "keys": [invalid]}),
        );
        reject_in_both_formats::<PermissionDefinition>(
            "invalid declared key",
            json!({"key": invalid, "description": "", "children": []}),
        );
        reject_in_both_formats::<PermissionDefinition>(
            "invalid child key",
            json!({"key": "ticket.hoge", "description": "", "children": [invalid]}),
        );
    }
    for (field, invalid) in [
        ("workspace", ""),
        ("workspace", "other workspace"),
        ("feature", ""),
        ("feature", "other\nfeature"),
    ] {
        let mut catalog: Value = serde_json::from_str(CATALOG_JSON).unwrap();
        if field == "workspace" {
            catalog["workspace"] = json!(invalid);
        } else {
            catalog["namespaces"][0]["feature"] = json!(invalid);
        }
        reject_in_both_formats::<PermissionCatalog>("invalid catalog identity", catalog);
    }
}

#[test]
fn grant_inputs_reject_payload_and_caller_supplied_authority_at_every_layer() {
    let assignment: Value = serde_json::from_str(ASSIGNMENT_JSON).unwrap();
    reject_authority_and_payload_fields::<PermissionAssignment>(assignment.clone());
    for reference in assignment["grants"].as_array().unwrap() {
        reject_authority_and_payload_fields::<PermissionReference>(reference.clone());
    }
    reject_authority_and_payload_fields::<NamespacePermissions>(
        assignment["grants"][1]["value"].clone(),
    );
    let request = json!({"assignment": assignment});
    let parsed: ValidatePermissionsRequest = serde_json::from_value(request.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), request);
    reject_authority_and_payload_fields::<ValidatePermissionsRequest>(request.clone());
    let mut nested = request;
    nested["assignment"]["workspace"] = json!("other-workspace");
    reject_in_both_formats::<ValidatePermissionsRequest>("nested workspace spoof", nested);
}

#[test]
fn assignment_fields_reject_untyped_grants_and_nonboolean_revocation() {
    for (field, invalid) in [
        ("grants", json!("ticket.hoge")),
        ("grants", json!([{"ticket.hoge": true}])),
        ("exclusions", json!({})),
        ("revoked", json!("false")),
    ] {
        let mut input = json!({"grants": [], "exclusions": [], "revoked": false});
        input[field] = invalid;
        reject_in_both_formats::<PermissionAssignment>(field, input);
    }
}

#[test]
fn declaration_inputs_reject_payload_and_caller_supplied_authority_at_every_layer() {
    let catalog: Value = serde_json::from_str(CATALOG_JSON).unwrap();
    let declaration = &catalog["namespaces"][0]["declaration"];
    reject_authority_and_payload_fields::<NamespaceDeclaration>(declaration.clone());
    reject_authority_and_payload_fields::<PermissionDefinition>(
        declaration["permissions"][0].clone(),
    );
    reject_authority_and_payload_fields::<SetContribution>(declaration["sets"][0].clone());
    // A catalog reports authenticated identities; declaration inputs cannot supply them.
    let mut input = declaration.clone();
    input["feature"] = json!("other-feature");
    reject_in_both_formats::<NamespaceDeclaration>("feature identity injection", input);
}
