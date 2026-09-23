use std::fs;
use std::path::Path;

use server_api::repository_openapi_typescript::{
    COMPANION_OUTPUT_PATH, INPUT_PATH, generate_companion_typescript,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut check = false;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--check" => check = true,
            "--help" | "-h" => {
                println!(
                    "Generate the bounded Companion API TypeScript artifact.\n\nUsage: cargo run -q -p server-api --example generate_companion_api_types -- [--check]"
                );
                return Ok(());
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let input_path = root.join(INPUT_PATH);
    let output_path = root.join(COMPANION_OUTPUT_PATH);
    let input = fs::read_to_string(&input_path)
        .map_err(|error| format!("failed to read {}: {error}", input_path.display()))?;
    let generated = generate_companion_typescript(&input)
        .map_err(|error| format!("failed to generate Companion API types: {error}"))?;
    if check {
        let checked_in = fs::read_to_string(&output_path)
            .map_err(|error| format!("failed to read {}: {error}", output_path.display()))?;
        if checked_in != generated {
            return Err(format!(
                "{} is stale; regenerate it without --check",
                output_path.display()
            ));
        }
        println!("{} is current", output_path.display());
        return Ok(());
    }
    let temporary_path = output_path.with_extension("ts.tmp");
    fs::write(&temporary_path, generated)
        .map_err(|error| format!("failed to write {}: {error}", temporary_path.display()))?;
    fs::rename(&temporary_path, &output_path)
        .map_err(|error| format!("failed to replace {}: {error}", output_path.display()))?;
    println!("generated {}", output_path.display());
    Ok(())
}
