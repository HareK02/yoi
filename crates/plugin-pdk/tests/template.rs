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
const TEMPLATE_README: &str =
    include_str!("../../../resources/plugin/templates/rust-component-tool/README.md");
const SERVICE_TEMPLATE_CARGO: &str =
    include_str!("../../../resources/plugin/templates/rust-component-instance/Cargo.toml");
const SERVICE_TEMPLATE_LIB: &str =
    include_str!("../../../resources/plugin/templates/rust-component-instance/src/lib.rs");
const SERVICE_TEMPLATE_PLUGIN: &str =
    include_str!("../../../resources/plugin/templates/rust-component-instance/plugin.toml");
const SERVICE_TEMPLATE_README: &str =
    include_str!("../../../resources/plugin/templates/rust-component-instance/README.md");
const SAMPLE_LIB: &str = include_str!("../../../docs/examples/plugin-component-tool/lib.rs");
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
    assert!(TEMPLATE_README.contains("Component Model Tool Plugin"));
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
    assert!(SERVICE_TEMPLATE_README.contains("output command"));
    assert!(!SERVICE_TEMPLATE_PLUGIN.contains("kind = \"wasm\""));
}

#[test]
fn documented_sample_uses_pdk_component_path() {
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
    let template_dir = crate_dir.join(format!("../../resources/plugin/templates/{template}"));
    let manifest_path = template_dir.join("Cargo.toml");
    let lock_path = template_dir.join("Cargo.lock");
    let _ = fs::remove_file(&lock_path);

    let target_dir = tempfile::tempdir().expect("temporary cargo target dir");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .arg("check")
        .arg("--manifest-path")
        .arg(&manifest_path)
        .arg("--target")
        .arg("wasm32-unknown-unknown")
        .arg("--offline")
        .arg("--target-dir")
        .arg(target_dir.path())
        .env("CARGO_TERM_COLOR", "never")
        .output()
        .expect("spawn cargo check for embedded template");

    let _ = fs::remove_file(&lock_path);
    assert!(
        output.status.success(),
        "template cargo check failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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
        "llm-engine",
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
