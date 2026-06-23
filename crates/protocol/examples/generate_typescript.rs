#[cfg(feature = "typescript")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = protocol::typescript::generated_typescript_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, protocol::typescript::generated_protocol_types())?;
    println!("wrote {}", path.display());
    Ok(())
}

#[cfg(not(feature = "typescript"))]
fn main() {
    eprintln!("enable the `typescript` feature to generate protocol TypeScript bindings");
    std::process::exit(2);
}
