//! The generated Web contract is an external artifact, not a second hand-written
//! list of domains. Normal crate tests reject drift from the declaration types.
#[test]
fn checked_in_web_contract_matches_the_payload_free_rust_types() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../web/workspace/src/lib/generated/feature-permissions.generated.ts");
    let saved = std::fs::read_to_string(path).expect("run the generate_typescript example");
    assert_eq!(saved, feature_permissions::typescript());
}
