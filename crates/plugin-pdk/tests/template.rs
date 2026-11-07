//! Real Rust -> core Wasm -> Component coverage for the shipped starters.
//! Only filesystem paths are relocated; WIT and guest source are unchanged.
//! Requires the wasm32-unknown-unknown Rust std target and cached Cargo dependencies.
//! No guest is instantiated and no HTTPS Host is faked by this suite.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use toml::Value;
use wit_parser::WorldItem;

const TEMPLATES: [&str; 2] = ["rust-component-tool", "rust-component-https"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn template_wit_copies_match_the_versioned_contract() {
    let root = root();
    for template in TEMPLATES {
        for file in ["yoi-plugin-v2.wit", "deps/yoi-host/yoi-host-v2.wit"] {
            assert_eq!(
                fs::read(root.join("resources/plugin/wit").join(file)).unwrap(),
                fs::read(
                    root.join("resources/plugin/templates")
                        .join(template)
                        .join("wit")
                        .join(file)
                )
                .unwrap(),
                "{template}/{file} must be regenerated from the canonical WIT"
            );
        }
    }
}

#[test]
fn template_manifest_worlds_use_exact_versions_and_schemas() {
    for template in TEMPLATES {
        let path = root().join("resources/plugin/templates").join(template);
        let cargo: Value =
            toml::from_str(&fs::read_to_string(path.join("Cargo.toml")).unwrap()).unwrap();
        assert!(cargo["workspace"].is_table());
        assert_eq!(cargo["lib"]["crate-type"][0].as_str(), Some("cdylib"));
        let manifest: Value =
            toml::from_str(&fs::read_to_string(path.join("plugin.toml")).unwrap()).unwrap();
        assert_eq!(manifest["schema_version"].as_integer(), Some(2));
        assert_eq!(manifest["runtime"]["kind"].as_str(), Some("wasm-component"));
        assert!(
            manifest["runtime"]["world"]
                .as_str()
                .unwrap()
                .ends_with("@2.0.0")
        );
        for tool in manifest["tools"].as_array().unwrap() {
            for schema in ["input_schema", "output_schema"] {
                assert_eq!(tool[schema]["type"].as_str(), Some("object"));
                assert_eq!(tool[schema]["additionalProperties"].as_bool(), Some(false));
            }
        }
    }
}

#[test]
fn pure_starter_builds_real_component_without_imports() {
    build_component(
        "rust-component-tool",
        "yoi_rust_component_tool_template",
        &[],
    );
}

#[test]
fn https_starter_builds_real_component_with_only_explicit_host_imports() {
    build_component(
        "rust-component-https",
        "yoi_rust_component_https_template",
        &[
            "yoi:host/connections@2.0.0",
            "yoi:host/errors@2.0.0",
            "yoi:host/http@2.0.0",
        ],
    );
}

fn build_component(template: &str, library: &str, expected_imports: &[&str]) {
    let root = root();
    let sdk = root.join("target/sdk");
    fs::create_dir_all(&sdk).unwrap();
    let source = root.join("resources/plugin/templates").join(template);
    let fixture = tempfile::Builder::new()
        .prefix("build-")
        .tempdir_in(&sdk)
        .unwrap();
    copy_tree(&source, fixture.path());
    let cargo_path = fixture.path().join("Cargo.toml");
    let cargo = fs::read_to_string(&cargo_path).unwrap().replace(
        "../../../../crates/plugin-pdk",
        root.join("crates/plugin-pdk").to_str().unwrap(),
    );
    fs::write(&cargo_path, cargo).unwrap();
    let target = sdk.join("guest");
    let output = Command::new(env!("CARGO"))
        .args([
            "build",
            "--release",
            "--offline",
            "--target",
            "wasm32-unknown-unknown",
            "--target-dir",
        ])
        .arg(&target)
        .arg("--manifest-path")
        .arg(&cargo_path)
        .current_dir(fixture.path())
        .env("CARGO_TERM_COLOR", "never")
        .output()
        .expect("run guest Cargo build");
    let logs = sdk.join("logs");
    fs::create_dir_all(&logs).unwrap();
    fs::write(logs.join(format!("{template}.stdout.log")), &output.stdout).unwrap();
    fs::write(logs.join(format!("{template}.stderr.log")), &output.stderr).unwrap();
    assert!(
        output.status.success(),
        "{template}: cargo build failed\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let core =
        fs::read(target.join(format!("wasm32-unknown-unknown/release/{library}.wasm"))).unwrap();
    let component = wit_component::ComponentEncoder::default()
        .module(&core)
        .unwrap()
        .validate(true)
        .encode()
        .unwrap();
    wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&component)
        .unwrap();
    let wit_parser::decoding::DecodedWasm::Component(resolve, world) =
        wit_component::decode(&component).unwrap()
    else {
        panic!("expected Component, not WIT package")
    };
    let world = &resolve.worlds[world];
    let mut imports: Vec<_> = world
        .imports
        .keys()
        .map(|key| resolve.name_world_key(key))
        .collect();
    imports.sort();
    assert_eq!(imports, expected_imports);
    let exports: Vec<_> = world
        .exports
        .keys()
        .map(|key| resolve.name_world_key(key))
        .collect();
    assert_eq!(exports, ["yoi:plugin/tool@2.0.0"]);
    let WorldItem::Interface { id, .. } = world.exports.values().next().unwrap() else {
        panic!("Tool interface export required")
    };
    assert_eq!(
        resolve.interfaces[*id].functions.keys().collect::<Vec<_>>(),
        [&"invoke".to_string()]
    );

    // Retain actual package inputs for the shared validator/pack owner, not a mock .wasm.
    let package = sdk.join("fixtures").join(template);
    fs::create_dir_all(&package).unwrap();
    fs::copy(source.join("plugin.toml"), package.join("plugin.toml")).unwrap();
    fs::write(package.join("plugin.component.wasm"), component).unwrap();
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        assert!(!kind.is_symlink(), "template may not contain symlinks");
        let name = entry.file_name();
        if name == "target" || name == "Cargo.lock" {
            continue;
        }
        let copied = destination.join(name);
        if kind.is_dir() {
            copy_tree(&entry.path(), &copied);
        } else {
            fs::copy(entry.path(), copied).unwrap();
        }
    }
}

#[test]
fn pdk_runtime_dependencies_are_guest_side_only() {
    let cargo: Value = toml::from_str(include_str!("../Cargo.toml")).unwrap();
    for dependency in cargo["dependencies"].as_table().unwrap().keys() {
        assert!(
            ["serde", "serde_json", "thiserror", "wit-bindgen"].contains(&dependency.as_str()),
            "unexpected Host dependency {dependency}"
        );
    }
}
