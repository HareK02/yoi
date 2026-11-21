//! Public MR event additions must be wired into business routing (or explicitly excluded).
use super::*;

#[test]
fn notification_registration_covers_public_mr_event_kinds() {
    let document = server_api::canonical_openapi_document()
        .unwrap()
        .to_json()
        .unwrap();
    let document: serde_json::Value = serde_json::from_str(&document).unwrap();
    let variants = document["components"]["schemas"]["MergeRequestThreadEvent"]["oneOf"]
        .as_array()
        .expect("public tagged MR event schema");
    assert!(!variants.is_empty());
    for variant in variants {
        let kind = variant["properties"]["kind"]["const"]
            .as_str()
            .expect("MR event kind discriminator");
        assert_ne!(
            NotificationEventKind::from_source(&format!("mr_{kind}")),
            NotificationEventKind::Unknown,
            "new public MR event {kind} needs a routing registration and source/receipt integration test"
        );
    }
    for kind in [
        "implementation_result",
        "mr_source_changed",
        "mr_publication_unconfirmed",
        "mr_publication_not_published",
        "mr_publication_published",
    ] {
        assert_ne!(
            NotificationEventKind::from_source(kind),
            NotificationEventKind::Unknown,
            "{kind} adapter is disconnected from routing"
        );
    }
}
