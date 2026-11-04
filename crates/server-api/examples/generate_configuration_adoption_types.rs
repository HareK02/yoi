#[cfg(feature="typescript")]
fn main() {
    print!("{}",server_api::normalized_typescript(server_api::configuration_adoption::configuration_typescript()));
}
#[cfg(not(feature="typescript"))]
fn main() {
    eprintln!("generate_configuration_adoption_types requires --features typescript");
    std::process::exit(2);
}
