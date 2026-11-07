//! Contract-boundary regressions. Public WIP values are converted by production contracts and
//! decoded by shared command DTOs; no Tool Schema is used as the expected native contract.
use super::*;

fn arguments(value: Value) -> BTreeMap<String, WipValue> {
    match json_to_wip(&value).unwrap() {
        WipValue::Record(fields) => fields,
        _ => panic!("record"),
    }
}
fn converted(op: &str, value: Value, bound: Option<&str>) -> Value {
    input(
        op,
        &arguments(value),
        bound,
        bound.map(|_| "ticket"),
        "/cases",
        "/changes",
        "/goals",
    )
    .unwrap_or_else(|_| panic!("native {op} rejected"))
}

#[test]
fn closure_judgment_compound_union_preserves_domain_reason_and_observed_null() {
    for reason in [
        json!({"$case":"not_planned"}),
        json!({"$case":"other","value":"Bounded research returned"}),
    ] {
        let command = converted(
            "case_update",
            json!({
                "operation_key":"judgment-1",
                "observed":{"content_digest":"digest","state":"open","closure_key":{"$case":"null"}},
                "action":{"$case":"close","value":{"reason":reason.clone()}},
                "explanation":"An explicit judgment"
            }),
            Some("T-42"),
        );
        let decoded: WorkspaceTicketCaseUpdateInput =
            serde_json::from_value(command.clone()).unwrap();
        assert_eq!(decoded.ticket, "T-42");
        assert_eq!(decoded.expected_closure_operation_key, None);
        let expected = if reason["$case"] == "other" {
            ticket::TicketClosureReason::Other("Bounded research returned".into())
        } else {
            ticket::TicketClosureReason::NotPlanned
        };
        assert_eq!(
            decoded.action,
            ticket::TicketCaseAction::Close { reason: expected }
        );
        assert_eq!(command["expected_closure_operation_key"], Value::Null);
    }
    let base = json!({"operation_key":"j","observed":{"content_digest":"d","state":"closed","closure_key":{"$case":"value","value":"previous"}},"action":{"$case":"reopen"},"explanation":"Handle again"});
    let command = converted("case_update", base.clone(), Some("T-42"));
    let decoded: WorkspaceTicketCaseUpdateInput = serde_json::from_value(command).unwrap();
    assert_eq!(decoded.action, ticket::TicketCaseAction::Reopen);
    assert_eq!(
        decoded.expected_closure_operation_key.as_deref(),
        Some("previous")
    );
    for invalid in [
        {
            let mut v = base.clone();
            v["observed"].as_object_mut().unwrap().remove("closure_key");
            v
        },
        {
            let mut v = base.clone();
            v["observed"]["closure_key"] = Value::Null;
            v
        },
        {
            let mut v = base.clone();
            v["action"] = json!({"$case":"reopen","value":null});
            v
        },
        {
            let mut v = base.clone();
            v["action"] =
                json!({"$case":"close","value":{"reason":{"$case":"completed","value":"extra"}}});
            v
        },
        {
            let mut v = base.clone();
            v["ticket"] = json!("T-99");
            v
        },
        {
            let mut v = base;
            v["observed"]["state"] = json!("done");
            v
        },
    ] {
        assert!(
            input(
                "case_update",
                &arguments(invalid),
                Some("T-42"),
                Some("ticket"),
                "/cases",
                "/changes",
                "/goals"
            )
            .is_err()
        );
    }
}

#[test]
fn editing_choices_and_implementation_targets_produce_closed_domain_commands() {
    let command = converted(
        "edit",
        json!({"body":{"$case":"splice","value":{"old":"before","new":"after"}},"targets":{"$case":"set","value":{"targets":[{"repository_key":"main","ref_selector":{"$case":"null"},"access":"read_write"}]}}}),
        Some("T-42"),
    );
    assert_eq!(
        command,
        json!({"ticket":"T-42","old_string":"before","new_string":"after","targets":{"action":"set","targets":[{"repository_key":"main","ref_selector":null,"access":"read_write"}]}})
    );
    assert_eq!(
        converted(
            "edit",
            json!({"body":{"$case":"replace","value":{"text":"Full body"}},"targets":{"$case":"clear"}}),
            Some("T-42")
        ),
        json!({"ticket":"T-42","body":"Full body","targets":{"action":"clear"}})
    );
    let omitted = converted("create", json!({"content":{"title":"Intent"}}), None);
    assert!(omitted.get("body").is_none());
    for value in [
        json!({"content":{"title":"Intent","body":null}}),
        json!({"content":{"title":"Intent","actor":"forged"}}),
        json!({"content":{"title":"Intent"},"targets":[{"repository_key":"main","access":"write"}]}),
    ] {
        assert!(
            input(
                "create",
                &arguments(value),
                None,
                None,
                "/cases",
                "/changes",
                "/goals"
            )
            .is_err()
        );
    }
}

#[test]
fn native_entry_inputs_use_allocated_namespace_and_never_subject_argument_ids() {
    assert_eq!(
        converted(
            "record_relation",
            json!({"kind":"depends_on","target":"/cases/T-9"}),
            Some("T-42")
        ),
        json!({"ticket":"T-42","kind":"depends_on","target":"T-9"})
    );
    for target in [
        "T-9",
        "/tickets/T-9",
        "/cases-extra/T-9",
        "/changes/T-9",
        "/cases/O-9",
        "/cases/T-9/child",
        "/cases/T-9%2Fchild",
    ] {
        assert!(
            input(
                "record_relation",
                &arguments(json!({"kind":"depends_on","target":target})),
                Some("T-42"),
                Some("ticket"),
                "/cases",
                "/changes",
                "/goals"
            )
            .is_err(),
            "{target}"
        );
    }
    assert!(
        input(
            "record_relation",
            &arguments(json!({"kind":"depends_on","target":"/cases/T-9"})),
            Some("T-42"),
            Some("ticket"),
            "",
            "/changes",
            "/goals"
        )
        .is_err()
    );
}

#[test]
fn assignment_principal_and_cas_are_declared_compounds_not_opaque_json() {
    let value = json!({"principal":{"$case":"subject","value":{"subject_id":"subject-a"}},"observed":{"assignment_id":{"$case":"null"}},"operation_id":"assign-1"});
    let command = converted("orchestrator_assign", value.clone(), Some("T-42"));
    assert_eq!(
        command,
        json!({"ticket":"T-42","principal":{"kind":"subject","subject_id":"subject-a"},"expected_assignment_id":null,"operation_id":"assign-1"})
    );
    for legacy in [
        json!({"$case":"worker","value":{"runtime_id":"r","worker_id":"w"}}),
        json!({"$case":"user","value":{"account_id":"a"}}),
        json!({"$case":"workspace_agent","value":{"agent_key":"workspace-orchestrator"}}),
    ] {
        let mut assignment = value.clone();
        assignment["principal"] = legacy;
        assert!(
            input(
                "orchestrator_assign",
                &arguments(assignment),
                Some("T-42"),
                Some("ticket"),
                "/cases",
                "/changes",
                "/goals"
            )
            .is_err()
        );
    }
    for invalid in [
        {
            let mut v = value.clone();
            v["principal"]["value"]["account_id"] = json!("spoof");
            v
        },
        {
            let mut v = value.clone();
            v["principal"] = json!({"$case":"admin"});
            v
        },
        {
            let mut v = value;
            v["observed"] = json!({});
            v
        },
    ] {
        assert!(
            input(
                "orchestrator_assign",
                &arguments(invalid),
                Some("T-42"),
                Some("ticket"),
                "/cases",
                "/changes",
                "/goals"
            )
            .is_err()
        );
    }
    let declaration = contract("orchestrator_assign").declaration(&ReferenceRoots {
        ticket: "/cases",
        merge_request: "/changes",
        objective: "/goals",
    });
    assert!(
        declaration
            .parameters
            .iter()
            .any(|p| p.name == "principal"
                && matches!(p.r#type, wip_protocol::TypeExpr::Union { .. }))
    );
    assert!(
        declaration
            .parameters
            .iter()
            .all(|p| !matches!(p.r#type, wip_protocol::TypeExpr::Json))
    );
}
