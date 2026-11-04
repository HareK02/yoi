fn main() -> Result<(), Box<dyn std::error::Error>> {
    print!(
        "{}",
        server_api::feature_permissions::permission_contract_openapi()?.to_json()?
    );
    Ok(())
}
