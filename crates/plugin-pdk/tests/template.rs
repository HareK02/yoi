//! Check the shipped templates against the real PDK and WIT for the guest target.
//! Each check owns its source and lockfile; Cargo alone shares and validates artifacts.

use std::fs;
use std::path::Path;
use std::process::Command;

use toml::Value;

const TEMPLATE_CARGO: &str =
    include_str!("../../../resources/plugin/templates/rust-component-tool/Cargo.toml");
const TEMPLATE_LIB: &str =
    include_str!("../../../resources/plugin/templates/rust-component-tool/src/lib.rs");
const TEMPLATE_PLUGIN: &str =
    include_str!("../../../resources/plugin/templates/rust-component-tool/plugin.toml");
const SERVICE_TEMPLATE_CARGO: &str =
    include_str!("../../../resources/plugin/templates/rust-component-instance/Cargo.toml");
const SERVICE_TEMPLATE_LIB: &str =
    include_str!("../../../resources/plugin/templates/rust-component-instance/src/lib.rs");
const SERVICE_TEMPLATE_PLUGIN: &str =
    include_str!("../../../resources/plugin/templates/rust-component-instance/plugin.toml");
// Preserve the removed document's source as a byte-identical fixture to keep the test input unchanged.
const SAMPLE_LIB: &str = include_str!("fixtures/component-tool-source.txt");
const PDK_CARGO: &str = include_str!("../Cargo.toml");

#[test]
fn rust_component_tool_template_has_expected_files() {
    let cargo: Value = toml::from_str(TEMPLATE_CARGO).expect("template Cargo.toml parses");
    assert_eq!(cargo["package"]["edition"].as_str(), Some("2024"));
    assert!(cargo["workspace"].is_table());
    assert_eq!(cargo["lib"]["crate-type"][0].as_str(), Some("cdylib"));
    assert_eq!(
        cargo["dependencies"]["yoi-plugin-pdk"]["path"].as_str(),
        Some("../../../../crates/plugin-pdk")
    );
    assert!(TEMPLATE_CARGO.contains("rev = \"<pinned-yoi-commit-sha>\""));

    let plugin: Value = toml::from_str(TEMPLATE_PLUGIN).expect("template plugin.toml parses");
    assert_eq!(plugin["schema_version"].as_integer(), Some(1));
    assert_eq!(plugin["runtime"]["kind"].as_str(), Some("wasm-component"));
    assert_eq!(
        plugin["runtime"]["world"].as_str(),
        Some("yoi:plugin/tool@1.0.0")
    );
    assert!(plugin["tools"].as_array().expect("tools array").len() == 1);

    assert!(TEMPLATE_LIB.contains("use yoi_plugin_pdk::wit_bindgen"));
    assert!(TEMPLATE_LIB.contains("wit_bindgen::generate!"));
    assert!(TEMPLATE_LIB.contains("generate_all"));
    assert!(TEMPLATE_LIB.contains("runtime_path: \"yoi_plugin_pdk::wit_bindgen::rt\""));
    assert!(TEMPLATE_LIB.contains("yoi_plugin_pdk::export_component_tool!"));
    assert!(TEMPLATE_LIB.contains("ToolOutput::json"));
}

#[test]
fn rust_component_service_template_has_event_output_pattern() {
    let cargo: Value = toml::from_str(SERVICE_TEMPLATE_CARGO).expect("service Cargo.toml parses");
    assert_eq!(cargo["package"]["edition"].as_str(), Some("2024"));
    assert_eq!(cargo["lib"]["crate-type"][0].as_str(), Some("cdylib"));
    assert_eq!(
        cargo["dependencies"]["yoi-plugin-pdk"]["path"].as_str(),
        Some("../../../../crates/plugin-pdk")
    );
    assert!(SERVICE_TEMPLATE_CARGO.contains("rev = \"<pinned-yoi-commit-sha>\""));

    let plugin: Value =
        toml::from_str(SERVICE_TEMPLATE_PLUGIN).expect("service plugin.toml parses");
    assert_eq!(plugin["schema_version"].as_integer(), Some(1));
    assert_eq!(plugin["runtime"]["kind"].as_str(), Some("wasm-component"));
    assert_eq!(
        plugin["runtime"]["world"].as_str(),
        Some("yoi:plugin/instance@1.0.0")
    );
    assert!(
        plugin["permissions"]
            .as_array()
            .expect("permissions array")
            .iter()
            .any(|permission| permission["kind"].as_str() == Some("host_api")
                && permission["api"].as_str() == Some("websocket"))
    );
    assert_eq!(
        plugin["services"].as_array().expect("services array").len(),
        1
    );
    let ingress = &plugin["ingresses"].as_array().expect("ingresses array")[0];
    assert!(
        ingress["event_kinds"]
            .as_array()
            .expect("event kinds")
            .iter()
            .any(|kind| kind.as_str() == Some("websocket_text"))
    );
    assert!(
        ingress["sources"]
            .as_array()
            .expect("sources")
            .iter()
            .any(|source| source.as_str() == Some("websocket:wss://example.com/socket"))
    );
    let websocket = &plugin["websocket"].as_array().expect("websocket targets")[0];
    assert_eq!(websocket["scheme"].as_str(), Some("wss"));
    assert_eq!(websocket["host"].as_str(), Some("example.com"));
    assert!(
        websocket["path_prefixes"]
            .as_array()
            .expect("websocket path prefixes")
            .iter()
            .any(|prefix| prefix.as_str() == Some("/socket"))
    );

    assert!(SERVICE_TEMPLATE_LIB.contains("world: \"instance\""));
    assert!(SERVICE_TEMPLATE_LIB.contains("PluginIngressEvent"));
    assert!(SERVICE_TEMPLATE_LIB.contains("ServiceOutput::websocket_send"));
    assert!(!SERVICE_TEMPLATE_LIB.contains("recv(timeout"));
    assert!(!SERVICE_TEMPLATE_PLUGIN.contains("kind = \"wasm\""));
}

#[test]
fn component_tool_fixture_uses_pdk_bindings() {
    assert!(SAMPLE_LIB.contains("use yoi_plugin_pdk::wit_bindgen"));
    assert!(SAMPLE_LIB.contains("wit_bindgen::generate!"));
    assert!(SAMPLE_LIB.contains("generate_all"));
    assert!(SAMPLE_LIB.contains("runtime_path: \"yoi_plugin_pdk::wit_bindgen::rt\""));
    assert!(SAMPLE_LIB.contains("yoi_plugin_pdk::export_component_tool!"));
    assert!(!SAMPLE_LIB.contains("export_name"));
}

#[test]
fn embedded_template_cargo_checks_for_wasm_target() {
    cargo_check_template("rust-component-tool");
}

#[test]
fn embedded_service_template_cargo_checks_for_wasm_target() {
    cargo_check_template("rust-component-instance");
}

fn cargo_check_template(template: &str) {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let templates_dir = crate_dir.join("../../resources/plugin/templates");
    let template_dir = templates_dir.join(template);

    // A sibling fixture preserves the shipped relative PDK and WIT paths verbatim.
    // Never create or remove a lockfile in the shipped template: another invocation
    // may be checking it at the same time, or a developer may own that lockfile.
    let fixture = tempfile::Builder::new()
        .prefix(".cargo-check-")
        .tempdir_in(&templates_dir)
        .expect("temporary template fixture");
    copy_template_tree(&template_dir, fixture.path(), true);

    // Both templates share the PDK dependencies. A separate persistent target
    // reuses those artifacts without sharing source or the outer Cargo build lock.
    // Cargo fingerprints and its target lock handle changes and concurrent checks.
    let executable = std::env::current_exe().expect("current test executable");
    let target_dir = executable
        .parent()
        .and_then(|deps| deps.parent())
        .expect("test executable has a profile directory")
        .join("compile-tests/plugin-pdk-templates");
    let output = Command::new(env!("CARGO"))
        .arg("check")
        .arg("--manifest-path")
        .arg(fixture.path().join("Cargo.toml"))
        .arg("--target")
        .arg("wasm32-unknown-unknown")
        .arg("--offline")
        .arg("--target-dir")
        .arg(target_dir)
        .current_dir(fixture.path())
        .env("CARGO_TERM_COLOR", "never")
        .output()
        .expect("spawn cargo check for embedded template");

    assert!(
        output.status.success(),
        "template {template} cargo check failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn copy_template_tree(source: &Path, destination: &Path, is_root: bool) {
    let metadata = fs::symlink_metadata(source).expect("read template directory metadata");
    assert!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "template input must be a real directory: {}",
        source.display()
    );
    for entry in fs::read_dir(source).expect("read template directory") {
        let entry = entry.expect("read template entry");
        let path = entry.path();
        let kind = entry.file_type().expect("read template entry type");
        assert!(
            !kind.is_symlink() && (kind.is_dir() || kind.is_file()),
            "unsupported template input (including symlinks): {}",
            path.display()
        );

        let name = entry.file_name();
        // Preserve all compiler inputs, including build scripts and nested assets.
        // Only omit the root build output and lockfile: each isolated consumer must
        // resolve its own lockfile, as the original unlocked template check did.
        if is_root
            && ((kind.is_dir() && name == "target") || (kind.is_file() && name == "Cargo.lock"))
        {
            continue;
        }
        let copied = destination.join(name);
        if kind.is_dir() {
            fs::create_dir(&copied).expect("create template fixture directory");
            copy_template_tree(&path, &copied, false);
        } else {
            fs::copy(&path, &copied).expect("copy unmodified template input");
        }
    }
}

#[test]
fn pdk_runtime_dependencies_are_guest_side_only() {
    let cargo: Value = toml::from_str(PDK_CARGO).expect("PDK Cargo.toml parses");
    let dependencies = cargo["dependencies"]
        .as_table()
        .expect("dependencies table");
    let forbidden = [
        "worker",
        "yoi-worker",
        "agen",
        "tui",
        "yoi-tui",
        "client",
        "yoi-client",
        "manifest",
        "yoi-manifest",
        "ticket",
        "yoi-ticket",
    ];

    for name in forbidden {
        assert!(
            !dependencies.contains_key(name),
            "PDK must not depend on host/runtime crate `{name}`"
        );
    }
}
