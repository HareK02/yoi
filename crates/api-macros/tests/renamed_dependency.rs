//! Compile the real public macro through a renamed dependency in an isolated consumer.
//! Only Cargo artifacts are shared; Cargo checks the fresh consumer on every invocation.

use std::{fs, path::PathBuf, process::Command};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        for attempt in 0..1000 {
            let path = std::env::temp_dir().join(format!(
                "api-macros-renamed-dependency-{}-{attempt}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create renamed dependency fixture: {error}"),
            }
        }
        panic!("could not allocate a renamed dependency fixture directory");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn macro_expansion_resolves_a_renamed_support_dependency() {
    let fixture = Fixture::new();
    let fixture = &fixture.0;
    fs::create_dir(fixture.join("src")).expect("create fixture source directory");

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

    // Keep nested Cargo checks off the outer build's target lock. Cargo owns cache
    // invalidation and locking between concurrent consumers, not a saved
    // test-success marker.
    let executable = std::env::current_exe().expect("current test executable");
    let target_dir = executable
        .parent()
        .and_then(|deps| deps.parent())
        .expect("test executable has a profile directory")
        .join("compile-tests/api-macros-renamed");
    let output = Command::new(env!("CARGO"))
        .args(["check", "--quiet", "--offline"])
        .current_dir(fixture)
        .env("CARGO_TARGET_DIR", target_dir)
        .output()
        .expect("run cargo check for renamed dependency fixture");

    assert!(
        output.status.success(),
        "renamed dependency fixture failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
