use toml::Value;

const TEMPLATE_CARGO: &str =
    include_str!("../../../resources/plugin/templates/rust-component-tool/Cargo.toml");
const TEMPLATE_LIB: &str =
    include_str!("../../../resources/plugin/templates/rust-component-tool/src/lib.rs");
const TEMPLATE_PLUGIN: &str =
    include_str!("../../../resources/plugin/templates/rust-component-tool/plugin.toml");
const TEMPLATE_README: &str =
    include_str!("../../../resources/plugin/templates/rust-component-tool/README.md");
const SAMPLE_LIB: &str = include_str!("../../../docs/examples/plugin-component-tool/lib.rs");
const PDK_CARGO: &str = include_str!("../Cargo.toml");

#[test]
fn rust_component_tool_template_has_expected_files() {
    let cargo: Value = toml::from_str(TEMPLATE_CARGO).expect("template Cargo.toml parses");
    assert_eq!(cargo["package"]["edition"].as_str(), Some("2024"));
    assert_eq!(cargo["lib"]["crate-type"][0].as_str(), Some("cdylib"));
    assert_eq!(
        cargo["dependencies"]["yoi-plugin-pdk"]["path"].as_str(),
        Some("../../../../crates/plugin-pdk")
    );
    assert!(TEMPLATE_CARGO.contains("rev = \"<pinned-yoi-revision>\""));

    let plugin: Value = toml::from_str(TEMPLATE_PLUGIN).expect("template plugin.toml parses");
    assert_eq!(plugin["schema_version"].as_integer(), Some(1));
    assert_eq!(plugin["runtime"]["kind"].as_str(), Some("wasm-component"));
    assert_eq!(
        plugin["runtime"]["world"].as_str(),
        Some("yoi:plugin/tool@1.0.0")
    );
    assert!(plugin["tools"].as_array().expect("tools array").len() == 1);

    assert!(TEMPLATE_LIB.contains("yoi_plugin_pdk::wit_bindgen::generate!"));
    assert!(TEMPLATE_LIB.contains("yoi_plugin_pdk::export_component_tool!"));
    assert!(TEMPLATE_LIB.contains("ToolOutput::json"));
    assert!(TEMPLATE_README.contains("Component Model Tool Plugin"));
}

#[test]
fn documented_sample_uses_pdk_component_path() {
    assert!(SAMPLE_LIB.contains("yoi_plugin_pdk::wit_bindgen::generate!"));
    assert!(SAMPLE_LIB.contains("yoi_plugin_pdk::export_component_tool!"));
    assert!(!SAMPLE_LIB.contains("export_name"));
}

#[test]
fn pdk_runtime_dependencies_are_guest_side_only() {
    let cargo: Value = toml::from_str(PDK_CARGO).expect("PDK Cargo.toml parses");
    let dependencies = cargo["dependencies"]
        .as_table()
        .expect("dependencies table");
    let forbidden = [
        "pod",
        "yoi-pod",
        "llm-worker",
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
