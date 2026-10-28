//! Projection boundary tests: trusted Model fields, paging and references are external contracts.
use super::*;
use crate::wip::wip_to_json;

fn projected(operation: &str, value: Value, mr: &str, objective: &str) -> Value {
    wip_to_json(
        &project(operation, value, None, "/cases", mr, objective)
            .unwrap_or_else(|_| panic!("projection failed")),
    )
    .unwrap()
}

#[test]
fn query_entries_preserve_display_ids_optional_links_and_opaque_cursor() {
    let item = json!({"ticket":"T-42", "linked_objectives":["O-3"],
        "merge_requests":[{"merge_request_id":"MR-1"}], "merge_request":null,
        "snippet":"/cases/T-99; {\"ticket\":\"T-88\"}", "current_worker":{"worker":"T-777"}});
    let output = projected(
        "query",
        json!({"tickets":[item.clone()],"next_cursor":"/cases/T-999","has_more":true}),
        "/changes",
        "",
    );
    assert_eq!(output["next_cursor"], "/cases/T-999");
    assert_eq!(output["has_more"], true);
    let selected = &output["items"][0];
    assert_eq!(selected["entry"], "/cases/T-42");
    assert_eq!(selected["data"], item);
    assert_eq!(selected["links"]["tickets"], json!([]));
    assert_eq!(selected["links"]["objectives"], json!([{"id":"O-3"}]));
    assert_eq!(
        selected["links"]["merge_requests"],
        json!([{"id":"MR-1","entry":"/changes/MR-1"}])
    );
}

#[test]
fn empty_page_omits_null_cursor_but_preserves_empty_cursor_and_has_more() {
    assert_eq!(
        projected(
            "query",
            json!({"tickets":[],"next_cursor":null,"has_more":false}),
            "",
            ""
        ),
        json!({"items":[],"has_more":false})
    );
    assert_eq!(
        projected(
            "query",
            json!({"tickets":[],"next_cursor":"","has_more":true}),
            "",
            ""
        ),
        json!({"items":[],"has_more":true,"next_cursor":""})
    );
}

#[test]
fn detail_related_entries_use_only_explicit_projected_relationship_fields() {
    let data = json!({"ticket":"T-42", "body":"/cases/T-999", "linked_objectives":[{"objective":"O-3"}],
        "relations":{"incoming":[{"ticket":"T-1"}],"outgoing":[{"ticket":"T-2"}],"blockers":[{"ticket":"T-3"}],"notices":[{"kind":"hidden"}]},
        "merge_requests":[],"merge_request":{"merge_request_id":"MR-1"},
        "current_closure":{"references":[{"kind":"other","target":"T-999"}]}});
    let output = projected("read", data.clone(), "/changes", "/goals");
    assert_eq!(output["data"], data);
    assert_eq!(
        output["links"]["tickets"],
        json!([
            {"id":"T-1","entry":"/cases/T-1"},{"id":"T-2","entry":"/cases/T-2"},{"id":"T-3","entry":"/cases/T-3"}
        ])
    );
    assert_eq!(
        output["links"]["objectives"],
        json!([{"id":"O-3","entry":"/goals/O-3"}])
    );
    assert_eq!(
        output["links"]["merge_requests"],
        json!([{"id":"MR-1","entry":"/changes/MR-1"}])
    );
}

#[test]
fn creation_receipt_is_not_reinterpreted_as_a_complete_item_observation() {
    let output = projected("create", json!({"id":"T-42","state":"open"}), "", "");
    assert_eq!(output["entry"], "/cases/T-42");
    assert_eq!(output["data"], json!({"id":"T-42","state":"open"}));
    assert_eq!(
        output["links"],
        json!({"tickets":[],"objectives":[],"merge_requests":[]})
    );
    assert!(
        project(
            "create",
            json!({"id":"/cases/T-42"}),
            None,
            "/cases",
            "",
            ""
        )
        .is_err()
    );
}

#[test]
fn declared_ticket_results_validate_navigation_and_do_not_type_domain_strings_as_entries() {
    let descriptor = InterfaceDescriptor {
        format: INTERFACE_FORMAT_V1.into(),
        documentation: None,
        types: vec![],
        operations: ["query", "create", "read"]
            .into_iter()
            .map(|op| OperationDeclaration {
                name: op.into(),
                documentation: None,
                parameters: vec![],
                returns: declaration(op, op == "read").unwrap(),
            })
            .collect(),
    };
    descriptor.validate().unwrap();
    for (operation, data) in [
        ("create", json!({"id":"T-42","state":"open"})),
        (
            "read",
            json!({"ticket":"T-42","linked_objectives":["O-3"],"body":"not-an-entry","event_page":{"next_cursor":"/cases/T-99","has_more":true}}),
        ),
        (
            "query",
            json!({"tickets":[{"ticket":"T-42"}],"next_cursor":"/cases/T-99","has_more":true}),
        ),
    ] {
        let output = projected(operation, data, "/changes", "/goals");
        descriptor
            .validate_result(operation, &json_to_wip(&output).unwrap())
            .unwrap();
        let mut invalid = output.clone();
        if operation == "query" {
            invalid["items"][0]["entry"] = json!(42);
        } else {
            invalid["entry"] = json!("not-an-absolute-coordinate");
        }
        assert!(
            descriptor
                .validate_result(operation, &json_to_wip(&invalid).unwrap())
                .is_err()
        );
    }
}

#[test]
fn lookup_and_record_results_publish_only_their_explicit_domain_reference_fields() {
    let cases = [
        (
            "implementation_result_query",
            json!({"ticket_id":"T-42","request":{"merge_request":{"merge_request_id":"MR-1","subject_ref":"sha"},"body":"T-999"}}),
            vec!["T-42"],
            vec!["MR-1"],
        ),
        (
            "implementation_result",
            json!({"ticket_id":"T-42","request":{"merge_request":null}}),
            vec!["T-42"],
            vec![],
        ),
        (
            "assignment_tracking_query",
            json!([{"ticket_id":"T-42","tracking":{"result":"T-999","progress":"/cases/T-999"}}]),
            vec!["T-42"],
            vec![],
        ),
        (
            "assignment_tracking_update",
            json!({"ticket_id":"T-42","tracking":{"result":"T-999"}}),
            vec!["T-42"],
            vec![],
        ),
        (
            "assignments_query",
            json!({"items":[{"ticket_id":"T-42"}],"next_assignment_id":"T-999"}),
            vec!["T-42"],
            vec![],
        ),
        (
            "query_orchestration_plans",
            json!({"records":[{"ticket_id":"T-42","related_ticket":"T-7","accepted_plan":{"branch":"T-999","worktree":"/cases/T-999"}}],"truncated":true,"count":2,"returned":1}),
            vec!["T-42", "T-7"],
            vec![],
        ),
        (
            "record_orchestration_plan",
            json!({"ticket_id":"T-42","related_ticket":"T-7","note":"T-999"}),
            vec!["T-42", "T-7"],
            vec![],
        ),
        (
            "dependency_check",
            json!({"ticket":{"id":"internal42","resource_key":"T-42"},"blockers":[{"blocking_ticket":"T-7"}],"queue_tickets":["T-8"],"recommended_action":"wait_for_orchestrator"}),
            vec!["T-42", "T-7", "T-8"],
            vec![],
        ),
        (
            "record_relation",
            json!({"ticket_id":"T-42","target":"T-7"}),
            vec!["T-42", "T-7"],
            vec![],
        ),
        (
            "remove_relation",
            json!({"ticket_id":"T-42","target":"T-7"}),
            vec!["T-42", "T-7"],
            vec![],
        ),
        (
            "edit",
            json!({"meta":{"id":"T-42"},"relations":{"outgoing":[{"ticket_id":"T-42","target":"T-7"}],"incoming":[{"source_ticket":"T-8"}],"blockers":[{"blocking_ticket":"T-9"}],"notices":[{"related_ticket":"T-10"}]}}),
            vec!["T-10", "T-42", "T-7", "T-8", "T-9"],
            vec![],
        ),
        (
            "case_update",
            json!({"meta":{"id":"T-42"},"relations":{"outgoing":[{"ticket_id":"T-42","target":"T-7"}]},"current_closure":{"references":[{"kind":"other","target":"T-999"}]}}),
            vec!["T-42", "T-7"],
            vec![],
        ),
        (
            "notification_diagnostics",
            json!({"items":[{"ticket":"T-42","merge_request_id":"MR-1","job_id":"T-999"},{"ticket":null,"merge_request_id":null}],"next_cursor":"/cases/T-999","source_checks":[],"limit":2}),
            vec!["T-42"],
            vec!["MR-1"],
        ),
    ];
    for (operation, data, tickets, mrs) in cases {
        let notification = operation == "notification_diagnostics";
        let output = project(
            operation,
            data.clone(),
            (!notification).then_some("T-42"),
            "/cases",
            "/changes",
            "/goals",
        )
        .unwrap_or_else(|_| panic!("{operation}"));
        let descriptor = InterfaceDescriptor {
            format: INTERFACE_FORMAT_V1.into(),
            documentation: None,
            types: vec![],
            operations: vec![OperationDeclaration {
                name: operation.into(),
                documentation: None,
                parameters: vec![],
                returns: declaration(operation, !notification).unwrap(),
            }],
        };
        descriptor.validate_result(operation, &output).unwrap();
        let output = wip_to_json(&output).unwrap();
        assert_eq!(output["data"], data, "{operation}");
        assert_eq!(
            output.get("entry"),
            (!notification).then_some(&json!("/cases/T-42")),
            "{operation}"
        );
        let ids = |domain: &str| {
            output["links"][domain]
                .as_array()
                .unwrap()
                .iter()
                .map(|link| link["id"].as_str().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids("tickets"), tickets, "{operation}");
        assert_eq!(ids("merge_requests"), mrs, "{operation}");
    }
}

#[test]
fn assignment_event_and_tag_identifiers_do_not_create_child_objects_or_text_links() {
    for operation in [
        "assignment_tracking_history",
        "assignment_answer",
        "assignment_diagnostics",
        "tags_set",
        "comment",
    ] {
        let data = json!({"ticket":"T-42","assignment_id":"T-99","result_id":"T-98","event_ref":"/cases/T-97","items":[{"ticket":"T-96","source_context":{"merge_request_id":"MR-95"}}]});
        let output = wip_to_json(
            &project(
                operation,
                data.clone(),
                Some("T-42"),
                "/cases",
                "/changes",
                "/goals",
            )
            .unwrap_or_else(|_| panic!("{operation}")),
        )
        .unwrap();
        assert_eq!(output["data"], data);
        assert_eq!(output["entry"], "/cases/T-42");
        assert_eq!(
            output["links"],
            json!({"tickets":[],"objectives":[],"merge_requests":[]})
        );
    }
}

#[test]
fn diagnostics_mr_origin_references_are_independent_of_capture_and_item_page() {
    for items in [
        json!([]),
        json!([{"ticket":"T-42","merge_request_id":null}]),
    ] {
        for root in ["", "/changes"] {
            let data = json!({"items":items,"next_cursor":"opaque-MR-999",
            "source_checks":[
                {"resource_kind":"merge_request_events","reference":"MR-1","origin_present":true,"captured":false},
                {"resource_kind":"job_result","reference":"MR-2","origin_present":true,"captured":true},
                {"resource_kind":"source_event","reference":"mr-event:MR-3:event","origin_present":true,"captured":true},
                {"resource_kind":"future_kind","reference":"MR-4","origin_present":true,"captured":false}
            ]});
            let output = projected("notification_diagnostics", data.clone(), root, "");
            assert_eq!(output["data"], data);
            let expected = if root.is_empty() {
                json!([{"id":"MR-1"}])
            } else {
                json!([{"id":"MR-1","entry":"/changes/MR-1"}])
            };
            assert_eq!(output["links"]["merge_requests"], expected);
            assert!(output.get("entry").is_none());
            let descriptor = InterfaceDescriptor {
                format: INTERFACE_FORMAT_V1.into(),
                documentation: None,
                types: vec![],
                operations: vec![OperationDeclaration {
                    name: "notification_diagnostics".into(),
                    documentation: None,
                    parameters: vec![],
                    returns: declaration("notification_diagnostics", false).unwrap(),
                }],
            };
            descriptor
                .validate_result("notification_diagnostics", &json_to_wip(&output).unwrap())
                .unwrap();
        }
    }
}
