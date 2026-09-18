use std::{env, fs, io, path::PathBuf};

const DEFAULT_OUTPUT: &str = "openapi/server-api.json";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os().skip(1);
    let first = arguments.next();
    let (mode, output) = match first.as_deref().and_then(|value| value.to_str()) {
        Some("--check") => ("check", arguments.next().map(PathBuf::from)),
        Some("--stdout") => ("stdout", None),
        Some(_) => ("write", first.map(PathBuf::from)),
        None => ("write", None),
    };
    if arguments.next().is_some() {
        return Err("usage: export_openapi [--check [PATH] | --stdout | PATH]".into());
    }

    let output = output.unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT));
    let json = server_api::canonical_openapi_document()?.to_json()?;
    match mode {
        "stdout" => print!("{json}"),
        "check" => {
            let checked_in = fs::read_to_string(&output)?;
            if checked_in != json {
                return Err(format!(
                    "{} is stale; regenerate with `cargo run -p server-api --example export_openapi -- {}`",
                    output.display(),
                    output.display()
                )
                .into());
            }
            println!("{} is current", output.display());
        }
        "write" => {
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut temporary = output.clone();
            let extension = output
                .extension()
                .and_then(|value| value.to_str())
                .map_or_else(|| "tmp".to_owned(), |value| format!("{value}.tmp"));
            temporary.set_extension(extension);
            fs::write(&temporary, json)?;
            fs::rename(&temporary, &output).or_else(|error| {
                let _ = fs::remove_file(&temporary);
                Err::<(), io::Error>(error)
            })?;
            println!("wrote {}", output.display());
        }
        _ => unreachable!(),
    }
    Ok(())
}
