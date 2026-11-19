use serde_json::{Value, json};
use server_api::repository_openapi_typescript::generate_ticket_browser_typescript;

#[test]
fn generated_mr_contract_exposes_lifecycle_without_mr_aggregation() {
    let document = server_api::canonical_openapi_document()
        .unwrap()
        .to_json()
        .unwrap();
    let schema: Value = serde_json::from_str(&document).unwrap();
    let schemas = &schema["components"]["schemas"];
    let variants = schemas["MergeRequestState"]["oneOf"].as_array().unwrap();
    assert_eq!(variants.len(), 4);
    let integration = variants
        .iter()
        .find(|v| v["properties"]["kind"]["const"] == "integration")
        .unwrap();
    assert!(
        integration["required"]
            .as_array()
            .unwrap()
            .contains(&json!("started_by"))
    );
    for name in ["PublicMergeRequest", "TicketMergeRequestSummary"] {
        let properties = &schemas[name]["properties"];
        for field in ["state"] {
            assert!(properties.get(field).is_some(), "{name}.{field}");
        }
        for field in ["phase", "integration_started_by", "integration_sources"] {
            assert!(properties.get(field).is_none(), "{name}.{field}");
        }
    }
    for name in ["ReviewEvent", "MergeRequestReviewSummary"] {
        assert!(
            schemas[name]["properties"]
                .get("integration_source")
                .is_none(),
            "{name}"
        );
    }
    for kind in ["review_requested", "review"] {
        let variant = schemas["MergeRequestThreadEvent"]["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|variant| variant["properties"]["kind"]["const"] == kind)
            .unwrap();
        assert!(variant["properties"].get("integration_source").is_none());
    }
    {
        let typescript = generate_ticket_browser_typescript(&document).unwrap();
        for name in [
            "MergeRequestState",
            "MergeRequestCompletion",
            "MergeRequestCancellationOrigin",
        ] {
            assert!(
                typescript.contains(&format!("export type {name} =")),
                "{name}"
            );
        }
        assert!(!typescript.contains("integration_started_by"));
        assert!(!typescript.contains("MergeRequestPhase"));
        assert!(!typescript.contains("integration_sources"));
        for name in [
            "IntegrationSource",
            "IntegrationInputSnapshot",
            "IntegrationInput",
            "IntegrationDisposition",
        ] {
            assert!(schemas.get(name).is_none(), "{name}");
            assert!(!typescript.contains(&format!("export type {name} =")));
        }
    }
    assert!(
        schemas["OpenMergeRequestRequest"]["properties"]
            .get("integration_sources")
            .is_none()
    );
}

#[test]
fn lifecycle_wire_requires_variant_evidence_and_preserves_explicit_legacy_provenance() {
    for value in [
        json!({"kind":"implementation"}),
        json!({"kind":"integration","started_by":"approval"}),
        json!({"kind":"completed","completion":{"kind":"published","started_by":"approval","merge_event_id":"merge"}}),
        json!({"kind":"completed","completion":{"kind":"legacy","merge_event_id":null}}),
        json!({"kind":"canceled","from":{"kind":"implementation"}}),
        json!({"kind":"canceled","from":{"kind":"integration","started_by":"approval"}}),
        json!({"kind":"canceled","from":{"kind":"legacy"}}),
    ] {
        let domain: merge_request::MergeRequestState =
            serde_json::from_value(value.clone()).unwrap();
        let public = server_api::MergeRequestState::from(domain);
        assert_eq!(serde_json::to_value(&public).unwrap(), value);
        assert_eq!(
            serde_json::from_value::<server_api::MergeRequestState>(value).unwrap(),
            public
        );
    }
    for value in [
        json!("open"),
        json!({"kind":"closed","from":{"kind":"implementation"}}),
        json!({"kind":"integration"}),
        json!({"kind":"integration","started_by":null}),
        json!({"kind":"implementation","started_by":"approval"}),
        json!({"kind":"integration","started_by":"approval","completion":{}}),
        json!({"kind":"completed"}),
        json!({"kind":"completed","completion":{"kind":"published","merge_event_id":"merge"}}),
        json!({"kind":"completed","completion":{"kind":"legacy","started_by":"guessed"}}),
        json!({"kind":"canceled"}),
        json!({"kind":"canceled","from":{"kind":"integration"}}),
    ] {
        assert!(
            serde_json::from_value::<server_api::MergeRequestState>(value.clone()).is_err(),
            "{value}"
        );
    }
}
