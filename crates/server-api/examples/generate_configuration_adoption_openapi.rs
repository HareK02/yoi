fn main() -> Result<(),Box<dyn std::error::Error>> {
    print!("{}", server_api::configuration_adoption::configuration_contract_openapi()?.to_json()?);
    Ok(())
}
