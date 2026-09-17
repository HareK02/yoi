use std::{fs, path::PathBuf, process::Command};

#[test]
fn macro_expansion_resolves_a_renamed_support_dependency() {
    let fixture = std::env::temp_dir().join(format!(
        "api-macros-renamed-dependency-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&fixture);
    fs::create_dir_all(fixture.join("src")).expect("create fixture source directory");

    let package_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let package_path = package_path
        .to_str()
        .expect("package path is UTF-8")
        .replace('\\', "\\\\");
    fs::write(
        fixture.join("Cargo.toml"),
        format!(
            r#"[package]
name = "renamed-api-consumer"
version = "0.0.0"
edition = "2024"

[workspace]

[dependencies]
yoi-api = {{ package = "api-macros", path = "{package_path}" }}
"#,
        ),
    )
    .expect("write fixture manifest");
    fs::write(
        fixture.join("src/lib.rs"),
        r#"use yoi_api::{ApiContract, api};

pub struct Output;

#[api]
pub trait RenamedApi {
    #[get("/output", operation_id = "renamed.output")]
    async fn output(&self) -> Output;
}

pub fn operation_count() -> usize {
    <RenamedApiMetadata as ApiContract>::OPERATIONS.len()
}
"#,
    )
    .expect("write fixture source");

    let output = Command::new(env!("CARGO"))
        .args(["check", "--quiet", "--offline"])
        .current_dir(&fixture)
        .env("CARGO_TARGET_DIR", fixture.join("target"))
        .output()
        .expect("run cargo check for renamed dependency fixture");
    let _ = fs::remove_dir_all(&fixture);

    assert!(
        output.status.success(),
        "renamed dependency fixture failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
