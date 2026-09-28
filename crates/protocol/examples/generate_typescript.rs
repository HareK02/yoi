#[cfg(feature = "typescript")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let check = std::env::args()
        .skip(1)
        .any(|argument| argument == "--check");
    let outputs = [
        (
            protocol::typescript::generated_typescript_path(),
            protocol::typescript::generated_protocol_types(),
        ),
        (
            protocol::typescript::generated_runtime_validator_path(),
            protocol::typescript::generated_protocol_validator(),
        ),
    ];

    for (path, expected) in outputs {
        if check {
            let actual = std::fs::read_to_string(&path)?;
            if actual != expected {
                return Err(format!(
                    "generated protocol artifact is stale: {}; run without --check",
                    path.display()
                )
                .into());
            }
            println!("current {}", path.display());
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, expected)?;
            println!("wrote {}", path.display());
        }
    }
    Ok(())
}

#[cfg(not(feature = "typescript"))]
fn main() {
    eprintln!("enable the `typescript` feature to generate protocol TypeScript bindings");
    std::process::exit(2);
}
