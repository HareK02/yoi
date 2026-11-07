//! Native JSON boundary tests. Signed Inspect/Invoke plus persistence is exercised by the
//! Workspace server's configuration integration suite; these cases isolate representation drift.
use super::contract;
use crate::wip::{json_to_wip, wip_to_json};
use serde_json::{Value as Json, json};
use wip_protocol::{INTERFACE_FORMAT_V1, InterfaceDescriptor, TypeExpr, Value};

fn arguments(json: &Json) -> std::collections::BTreeMap<String, Value> {
    let Value::Record(arguments) = json_to_wip(json).unwrap() else {
        panic!("record expected")
    };
    arguments
}

#[test]
fn config_batch_native_union_maps_exact_backend_tagged_json() {
    for (case, payload) in [
        ("create", json!({"path":"new.dcdl", "content":"{}"})),
        (
            "create",
            json!({"path":"new.txt", "content":"note", "content_type":"text"}),
        ),
        (
            "create",
            json!({"path":"new.dcdl", "content":"{}", "content_type":"decodal"}),
        ),
        ("update", json!({"path":"existing.dcdl", "content":"{}"})),
        ("delete", json!({"path":"obsolete.txt"})),
        ("rename", json!({"from":"old.txt", "to":"new.txt"})),
    ] {
        let command = contract::convert(&arguments(
            &json!({"changes":[{"$case":case,"value":payload}]}),
        ))
        .unwrap_or_else(|_| panic!("published {case} shape must convert"));
        let mut expected = payload;
        expected["kind"] = json!(case);
        assert_eq!(command, json!({"changes":[expected]}));
        assert!(!command.to_string().contains("expected_digest"));
    }
}

#[test]
fn config_batch_native_shape_rejects_unpublished_fields_and_invalid_union_wire() {
    for change in [
        json!({"kind":"delete","path":"old.txt"}),
        json!({"$case":"unknown","value":{}}),
        json!({"$case":"create","value":{"path":"new.txt"}}),
        json!({"$case":"create","value":{"content":"note"}}),
        json!({"$case":"update","value":{"path":"old.txt"}}),
        json!({"$case":"delete","value":{}}),
        json!({"$case":"rename","value":{"from":"old.txt"}}),
        json!({"$case":"rename","value":{"to":"new.txt"}}),
        json!({"$case":"create","value":{"path":"new.txt","content":"note","content_type":"json"}}),
        json!({"$case":"create","value":{"path":"new.txt","content":"note","content_type":null}}),
        json!({"$case":"update","value":{"path":"old.txt","content":"note","content_type":"text"}}),
        json!({"$case":"delete","value":{"path":"old.txt","expected_digest":"manual"}}),
        json!({"$case":"delete","value":{"path":42}}),
        json!({"$case":"delete","value":{"path":"old.txt"},"kind":"delete"}),
    ] {
        assert!(
            contract::convert(&arguments(&json!({"changes":[change]}))).is_err(),
            "{change}"
        );
    }
    for input in [
        json!({}),
        json!({"changes":{}}),
        json!({"changes":[],"validator":"manual"}),
    ] {
        assert!(contract::convert(&arguments(&input)).is_err(), "{input}");
    }
}

#[test]
fn config_batch_declaration_exposes_structure_and_result_not_json() {
    let declaration = contract::declaration();
    let descriptor = InterfaceDescriptor {
        format: INTERFACE_FORMAT_V1.into(),
        documentation: None,
        types: vec![],
        operations: vec![declaration.clone()],
    };
    descriptor.validate().unwrap();
    let [parameter] = declaration.parameters.as_slice() else {
        panic!("one argument")
    };
    assert_eq!(parameter.name, "changes");
    assert!(parameter.required);
    let TypeExpr::List { items } = &parameter.r#type else {
        panic!("list")
    };
    let TypeExpr::Union { cases } = items.as_ref() else {
        panic!("union")
    };
    assert_eq!(
        cases
            .iter()
            .map(|case| case.name.as_str())
            .collect::<Vec<_>>(),
        ["create", "update", "delete", "rename"]
    );
    let signature = wip_text_view::render_interface(&wip_text_view::Interface {
        reference: &crate::wip::contextual_reference(
            "yoi.workspace-config/node/v1",
            "/workspace-config",
        ),
        descriptor: &descriptor,
    })
    .unwrap();
    for text in [
        "content_type?",
        "decodal",
        "text",
        "from:",
        "to:",
        "digest:",
        "activation:",
        "Whole-tree CAS",
        "OutcomeUnknown",
        "not hot application",
    ] {
        assert!(signature.contains(text), "missing {text}: {signature}");
    }
    // Native values remain ordinary JSON strings/records at the model boundary; no Entry inference.
    let result = json!({"digest":"tree-digest", "activation":contract::ACTIVATION});
    assert_eq!(wip_to_json(&json_to_wip(&result).unwrap()).unwrap(), result);
}
