//! Static validation of the exact, supported Plugin Component ABI.
//!
//! Component metadata is inferred from validated bytes, never trusted from a
//! producer's custom sections. This module does not instantiate guest code.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use wit_component::{DecodedWasm, WitPrinter};
use wit_parser::{InterfaceId, Resolve, WorldId, WorldItem, WorldKey};

const PLUGIN_WIT: &str = include_str!("../../../../resources/plugin/wit/yoi-plugin-v2.wit");
const HOST_WIT: &str =
    include_str!("../../../../resources/plugin/wit/deps/yoi-host/yoi-host-v2.wit");

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentMetadata {
    pub world: String,
    pub imports: Vec<String>,
    pub exports: Vec<String>,
}

/// Validates an executable Component against the embedded exact-version world.
///
/// `required_imports` is a set (duplicates are rejected); it must equal both the
/// selected world's imports and the actual Component imports. Returned names are
/// fully qualified and sorted. The world name comes from the supported contract,
/// not a producer-supplied custom section: executable Components do not encode
/// the original WIT world name.
pub fn validate_component(
    bytes: &[u8],
    world: &str,
    required_imports: &[String],
) -> Result<ComponentMetadata, String> {
    let world_name = match world {
        "yoi:plugin/tool-plugin@2.0.0" => "tool-plugin",
        "yoi:plugin/https-tool-plugin@2.0.0" => "https-tool-plugin",
        "yoi:plugin/logged-tool-plugin@2.0.0" => "logged-tool-plugin",
        "yoi:plugin/https-logged-tool-plugin@2.0.0" => "https-logged-tool-plugin",
        "yoi:plugin/ticket-tool-plugin@2.1.0" => "ticket-tool-plugin",
        _ => return Err(format!("unsupported Component world `{world}`")),
    };
    if !wasmparser::Parser::is_component(bytes) {
        return Err("expected a WebAssembly Component, not a core module".into());
    }
    wasmparser::Validator::new()
        .validate_all(bytes)
        .map_err(|error| format!("invalid WebAssembly Component: {error}"))?;
    let (actual, actual_world) = match wit_component::decode(bytes)
        .map_err(|error| format!("cannot decode Component interface: {error:#}"))?
    {
        DecodedWasm::Component(resolve, world) => (resolve, world),
        DecodedWasm::WitPackage(..) => {
            return Err("expected an executable Component, not a WIT type package".into());
        }
    };
    let (expected, expected_world) = contract(world_name)?;
    let expected_imports = surface(&expected, expected_world, true)?;
    let expected_exports = surface(&expected, expected_world, false)?;
    let actual_imports = surface(&actual, actual_world, true)?;
    let actual_exports = surface(&actual, actual_world, false)?;
    let required: BTreeSet<_> = required_imports.iter().cloned().collect();
    if required.len() != required_imports.len() {
        return Err("duplicate required Component import in manifest".into());
    }
    let imports: Vec<_> = expected_imports.keys().cloned().collect();
    let exports: Vec<_> = expected_exports.keys().cloned().collect();
    if required != expected_imports.keys().cloned().collect() {
        return Err(format!(
            "manifest required_imports do not match world `{world}`: expected {imports:?}"
        ));
    }
    if actual_imports.keys().ne(expected_imports.keys()) {
        return Err(format!(
            "Component imports do not match world `{world}`: expected {imports:?}, found {:?}",
            actual_imports.keys().collect::<Vec<_>>()
        ));
    }
    if actual_exports.keys().ne(expected_exports.keys()) {
        return Err(format!(
            "Component exports do not match world `{world}`: expected {exports:?}, found {:?}",
            actual_exports.keys().collect::<Vec<_>>()
        ));
    }

    // Compare all decoded interfaces, including those owning referenced types,
    // not only function names or the world surface. Printing preserves the
    // qualified owner of type aliases and resource identity, borrow/own, field
    // names/order, and nested signatures. Resolve::merge is not an ABI check.
    let expected_interfaces = named_interfaces(&expected)?;
    for (name, id) in named_interfaces(&actual)? {
        let expected_id = expected_interfaces
            .get(&name)
            .ok_or_else(|| format!("unsupported Component interface `{name}`"))?;
        if interface_text(&actual, id)? != interface_text(&expected, *expected_id)? {
            return Err(format!("Component interface `{name}` has an incorrect ABI"));
        }
    }

    Ok(ComponentMetadata {
        world: world.to_string(),
        imports,
        exports,
    })
}

fn contract(world_name: &str) -> Result<(Resolve, WorldId), String> {
    let mut resolve = Resolve::default();
    resolve
        .push_str("yoi-host-v2.wit", HOST_WIT)
        .map_err(|error| format!("invalid embedded Host WIT: {error:#}"))?;
    let old_package = resolve
        .push_str("yoi-plugin-v2.wit", PLUGIN_WIT)
        .map_err(|error| format!("invalid embedded Plugin WIT: {error:#}"))?;
    let package = if world_name == "ticket-tool-plugin" {
        resolve
            .push_str(
                "yoi-host-v2.1.wit",
                include_str!(
                    "../../../../resources/plugin/wit-ticket/deps/yoi-host-v2.1/yoi-host-v2.1.wit"
                ),
            )
            .map_err(|e| format!("invalid Ticket Host WIT: {e:#}"))?;
        resolve
            .push_str(
                "yoi-plugin-v2.1.wit",
                include_str!("../../../../resources/plugin/wit-ticket/yoi-plugin-v2.1.wit"),
            )
            .map_err(|e| format!("invalid Ticket Plugin WIT: {e:#}"))?
    } else {
        old_package
    };
    let world = resolve
        .select_world(&[package], Some(world_name))
        .map_err(|error| format!("invalid embedded Plugin world: {error:#}"))?;
    Ok((resolve, world))
}

fn surface(
    resolve: &Resolve,
    world: WorldId,
    imports: bool,
) -> Result<BTreeMap<String, InterfaceId>, String> {
    let items = if imports {
        &resolve.worlds[world].imports
    } else {
        &resolve.worlds[world].exports
    };
    let mut names = BTreeMap::new();
    for (key, item) in items {
        let (WorldKey::Interface(key_id), WorldItem::Interface { id, .. }) = (key, item) else {
            return Err(
                "Component world contains an unsupported bare function/type or named interface"
                    .into(),
            );
        };
        if key_id != id {
            return Err("Component interface identity does not match its world key".into());
        }
        let name = resolve
            .id_of(*id)
            .ok_or_else(|| "Component interface has no exact package identity".to_string())?;
        if names.insert(name.clone(), *id).is_some() {
            return Err(format!("duplicate Component interface `{name}`"));
        }
    }
    Ok(names)
}

fn named_interfaces(resolve: &Resolve) -> Result<BTreeMap<String, InterfaceId>, String> {
    let mut names = BTreeMap::new();
    for (id, _) in resolve.interfaces.iter() {
        let name = resolve
            .id_of(id)
            .ok_or_else(|| "Component interface has no exact package identity".to_string())?;
        if names.insert(name.clone(), id).is_some() {
            return Err(format!("duplicate Component interface `{name}`"));
        }
    }
    Ok(names)
}

fn interface_text(resolve: &Resolve, id: InterfaceId) -> Result<String, String> {
    // Declaration order is not ABI; record fields, variant cases, parameters,
    // and results remain ordered. Sorting maps also canonicalizes use aliases
    // and resource methods without changing their nominal owners.
    let mut resolve = resolve.clone();
    resolve.interfaces[id].types.sort_keys();
    resolve.interfaces[id].functions.sort_keys();
    let mut printer = WitPrinter::default();
    printer.emit_docs(false);
    printer
        .print_interface(&resolve, id)
        .map_err(|error| format!("cannot inspect Component ABI: {error:#}"))?;
    Ok(printer.output.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
    use wit_parser::ManglingAndAbi;

    const PURE: &str = "yoi:plugin/tool-plugin@2.0.0";
    const HTTPS: &str = "yoi:plugin/https-tool-plugin@2.0.0";
    const LOGGED: &str = "yoi:plugin/logged-tool-plugin@2.0.0";
    const HTTPS_LOGGED: &str = "yoi:plugin/https-logged-tool-plugin@2.0.0";

    fn fixture(host: &str, plugin: &str, world_name: &str) -> Vec<u8> {
        let mut resolve = Resolve::default();
        resolve.push_str("host.wit", host).unwrap();
        let package = resolve.push_str("plugin.wit", plugin).unwrap();
        let world = resolve.select_world(&[package], Some(world_name)).unwrap();
        encode(&resolve, world)
    }

    fn encode(resolve: &Resolve, world: WorldId) -> Vec<u8> {
        let mut module = dummy_module(resolve, world, ManglingAndAbi::Standard32);
        embed_component_metadata(&mut module, resolve, world, StringEncoding::UTF8).unwrap();
        ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .validate(true)
            .encode()
            .unwrap()
    }

    fn https_imports() -> Vec<String> {
        [
            "yoi:host/connections@2.0.0",
            "yoi:host/errors@2.0.0",
            "yoi:host/http@2.0.0",
        ]
        .map(str::to_string)
        .into()
    }

    fn logged_imports(https: bool) -> Vec<String> {
        let mut imports = if https {
            https_imports()
        } else {
            vec!["yoi:host/errors@2.0.0".into()]
        };
        imports.push("yoi:host/logging@2.0.0".into());
        imports
    }

    #[test]
    fn exact_world_components_return_sorted_metadata_without_running_guest() {
        for (name, world, imports) in [
            ("tool-plugin", PURE, vec![]),
            ("https-tool-plugin", HTTPS, https_imports()),
            ("logged-tool-plugin", LOGGED, logged_imports(false)),
            (
                "https-logged-tool-plugin",
                HTTPS_LOGGED,
                logged_imports(true),
            ),
        ] {
            let component = fixture(HOST_WIT, PLUGIN_WIT, name);
            assert_eq!(
                validate_component(&component, world, &imports).unwrap(),
                ComponentMetadata {
                    world: world.into(),
                    imports,
                    exports: vec!["yoi:plugin/tool@2.0.0".into()],
                }
            );
        }
    }

    #[test]
    fn logging_worlds_require_exact_manifest_and_actual_import_sets() {
        for (name, world, imports) in [
            ("logged-tool-plugin", LOGGED, logged_imports(false)),
            (
                "https-logged-tool-plugin",
                HTTPS_LOGGED,
                logged_imports(true),
            ),
        ] {
            let component = fixture(HOST_WIT, PLUGIN_WIT, name);
            let mut missing = imports.clone();
            missing.retain(|name| !name.contains("/logging@"));
            assert!(
                validate_component(&component, world, &missing)
                    .unwrap_err()
                    .contains("manifest required_imports")
            );
            let plugin = PLUGIN_WIT.replace("import yoi:host/logging@2.0.0;", "");
            let component = fixture(HOST_WIT, &plugin, name);
            assert!(
                validate_component(&component, world, &imports)
                    .unwrap_err()
                    .contains("Component imports")
            );
        }
    }

    #[test]
    fn logging_world_and_interface_versions_are_exact() {
        let logging = HOST_WIT
            .split_once("interface logging {")
            .unwrap()
            .1
            .replace(
                "use errors.{host-error};",
                "use yoi:host/errors@2.0.0.{host-error};",
            );
        for (name, world, imports) in [
            ("logged-tool-plugin", LOGGED, logged_imports(false)),
            (
                "https-logged-tool-plugin",
                HTTPS_LOGGED,
                logged_imports(true),
            ),
        ] {
            let supported = fixture(HOST_WIT, PLUGIN_WIT, name);
            for version in ["1.0.0", "2.0.1", "9.0.0"] {
                let unsupported_world = world.replace("@2.0.0", &format!("@{version}"));
                assert!(
                    validate_component(&supported, &unsupported_world, &imports)
                        .unwrap_err()
                        .contains("unsupported Component world")
                );
                let host = format!(
                    "{HOST_WIT}\npackage yoi:host@{version} {{ interface logging {{ {logging} }}"
                );
                let plugin = PLUGIN_WIT.replace("logging@2.0.0", &format!("logging@{version}"));
                let component = fixture(&host, &plugin, name);
                assert!(
                    validate_component(&component, world, &imports)
                        .unwrap_err()
                        .contains("Component imports")
                );
            }
        }
    }

    #[test]
    fn typed_logging_nested_payload_and_signature_changes_are_rejected() {
        for (from, to) in [
            (
                "trace, debug, info, warn, error",
                "debug, trace, info, warn, error",
            ),
            ("number(f64)", "number(f32)"),
            ("signed(s64)", "signed(u64)"),
            ("value: scalar", "value: string"),
            ("fields: list<field>", "fields: option<field>"),
            ("event-code: string", "event-name: string"),
            (
                "message: string,\n    fields:",
                "message: string, extra: bool,\n    fields:",
            ),
            ("log: func(input: entry)", "log: func(input: string)"),
            (
                "log: func(input: entry) -> result<_, host-error>",
                "log: func(input: entry) -> result<string, host-error>",
            ),
            ("log: func", "extra: func(); log: func"),
        ] {
            assert!(HOST_WIT.contains(from));
            let host = HOST_WIT.replace(from, to);
            for (name, world, imports) in [
                ("logged-tool-plugin", LOGGED, logged_imports(false)),
                (
                    "https-logged-tool-plugin",
                    HTTPS_LOGGED,
                    logged_imports(true),
                ),
            ] {
                let component = fixture(&host, PLUGIN_WIT, name);
                let error = validate_component(&component, world, &imports).unwrap_err();
                assert!(
                    error.contains("yoi:host/logging@2.0.0") && error.contains("incorrect ABI"),
                    "mutation: {from} -> {to}: {error}"
                );
            }
        }
    }

    #[test]
    fn validated_metadata_has_serializable_public_fields() {
        let component = fixture(HOST_WIT, PLUGIN_WIT, "logged-tool-plugin");
        let metadata = validate_component(&component, LOGGED, &logged_imports(false)).unwrap();
        let json = serde_json::to_value(&metadata).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "world": LOGGED,
                "imports": ["yoi:host/errors@2.0.0", "yoi:host/logging@2.0.0"],
                "exports": ["yoi:plugin/tool@2.0.0"],
            })
        );
        assert_eq!(
            serde_json::from_value::<ComponentMetadata>(json).unwrap(),
            metadata
        );
    }

    #[test]
    fn v1_unknown_and_wrong_manifest_worlds_are_rejected() {
        let component = fixture(HOST_WIT, PLUGIN_WIT, "tool-plugin");
        for world in [
            "yoi:plugin/tool-plugin@1.0.0",
            "yoi:plugin/tool-plugin@2.0.1",
        ] {
            assert!(
                validate_component(&component, world, &[])
                    .unwrap_err()
                    .contains("unsupported Component world")
            );
        }
        assert!(
            validate_component(&component, HTTPS, &https_imports())
                .unwrap_err()
                .contains("Component imports")
        );
        for version in ["1.0.0", "9.0.0"] {
            let changed = PLUGIN_WIT.replace("2.0.0", version);
            let changed_host = HOST_WIT.replace("2.0.0", version);
            let component = fixture(&changed_host, &changed, "tool-plugin");
            assert!(
                validate_component(&component, PURE, &[])
                    .unwrap_err()
                    .contains("Component exports")
            );
        }
    }

    #[test]
    fn wrong_host_interface_versions_are_not_semver_compatible() {
        for version in ["1.0.0", "2.0.1", "9.0.0"] {
            let host = HOST_WIT.replace("@2.0.0", &format!("@{version}"));
            let plugin = PLUGIN_WIT
                .replace("errors@2.0.0", &format!("errors@{version}"))
                .replace("connections@2.0.0", &format!("connections@{version}"))
                .replace("http@2.0.0", &format!("http@{version}"))
                .replace("logging@2.0.0", &format!("logging@{version}"));
            let component = fixture(&host, &plugin, "https-tool-plugin");
            assert!(
                validate_component(&component, HTTPS, &https_imports())
                    .unwrap_err()
                    .contains("Component imports")
            );
        }
    }

    #[test]
    fn manifest_import_set_must_equal_actual_and_supported_world() {
        let component = fixture(HOST_WIT, PLUGIN_WIT, "https-tool-plugin");
        let mut reordered = https_imports();
        reordered.reverse();
        assert!(validate_component(&component, HTTPS, &reordered).is_ok());
        let mut duplicate = https_imports();
        duplicate.push(duplicate[0].clone());
        for imports in [
            vec![],
            vec!["wasi:filesystem/types@0.2.0".into()],
            duplicate,
        ] {
            assert!(validate_component(&component, HTTPS, &imports).is_err());
        }
    }

    #[test]
    fn unadvertised_imports_and_extra_exports_are_rejected() {
        for (from, to) in [
            ("export tool;", ""),
            (
                "world tool-plugin {",
                "world tool-plugin { import surprise: func();",
            ),
            (
                "world tool-plugin {",
                "world tool-plugin { export surprise: func();",
            ),
            (
                "world tool-plugin {",
                "world tool-plugin { import yoi:host/logging@2.0.0;",
            ),
        ] {
            let plugin = PLUGIN_WIT.replace(from, to);
            let component = fixture(HOST_WIT, &plugin, "tool-plugin");
            assert!(validate_component(&component, PURE, &[]).is_err());
        }
        let plugin = PLUGIN_WIT.replace(
            "world tool-plugin {",
            "world tool-plugin { import wasi:unpublished/io@0.2.0;",
        );
        let host = format!(
            "{HOST_WIT}\npackage wasi:unpublished@0.2.0 {{ interface io {{ read: func(); }} }}"
        );
        let component = fixture(&host, &plugin, "tool-plugin");
        assert!(validate_component(&component, PURE, &[]).is_err());
    }

    #[test]
    fn same_interface_name_with_changed_nested_signature_is_rejected() {
        for (from, to) in [
            ("remaining-ms: u32", "remaining-ms: u64"),
            ("remaining-ms: u32", "time-left: u32"),
            ("content: option<string>", "content: list<string>"),
            ("summary: string", "summary: string, extra: bool"),
            ("input: invocation", "input: output"),
            ("result<output, tool-error>", "result<tool-error, output>"),
            ("invoke: func", "extra: func(); invoke: func"),
        ] {
            assert!(PLUGIN_WIT.contains(from));
            let plugin = PLUGIN_WIT.replace(from, to);
            let component = fixture(HOST_WIT, &plugin, "tool-plugin");
            assert!(
                validate_component(&component, PURE, &[])
                    .unwrap_err()
                    .contains("incorrect ABI"),
                "mutation: {from} -> {to}"
            );
        }
    }

    #[test]
    fn host_resource_identity_ownership_and_nested_records_are_exact() {
        for (from, to) in [
            ("borrow<connection>", "own<connection>"),
            ("use connections.{connection};", "resource connection;"),
            ("status: u16", "status: u32"),
            ("value: string", "value: list<u8>"),
            (
                "resource connection;",
                "resource connection; resource other;",
            ),
            ("target: borrow<connection>", "target: borrow<other>"),
            ("diagnostic-id: string", "diagnostic-id: u64"),
        ] {
            let mut host = HOST_WIT.replace(from, to);
            if to == "target: borrow<other>" {
                host = host.replace("interface http {", "interface http { resource other;");
            }
            assert_ne!(host, HOST_WIT);
            let component = fixture(&host, PLUGIN_WIT, "https-tool-plugin");
            assert!(
                validate_component(&component, HTTPS, &https_imports())
                    .unwrap_err()
                    .contains("incorrect ABI"),
                "mutation: {from} -> {to}"
            );
        }
    }

    #[test]
    fn interface_declaration_order_is_not_part_of_the_abi() {
        let (mut resolve, world) = contract("https-tool-plugin").unwrap();
        for (_, interface) in resolve.interfaces.iter_mut() {
            interface.types.reverse();
            interface.functions.reverse();
        }
        let component = encode(&resolve, world);
        assert!(validate_component(&component, HTTPS, &https_imports()).is_ok());
    }

    #[test]
    fn docs_are_not_part_of_the_abi() {
        let plugin = PLUGIN_WIT.replace(
            "interface tool {",
            "/// Different documentation.\ninterface tool {",
        );
        let component = fixture(HOST_WIT, &plugin, "tool-plugin");
        assert!(validate_component(&component, PURE, &[]).is_ok());
    }

    #[test]
    fn core_module_wit_type_package_and_invalid_component_are_rejected() {
        let (resolve, world) = contract("tool-plugin").unwrap();
        let module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
        assert!(
            validate_component(&module, PURE, &[])
                .unwrap_err()
                .contains("core module")
        );
        let package = resolve.worlds[world].package.unwrap();
        let types = wit_component::encode(&resolve, package).unwrap();
        assert!(
            validate_component(&types, PURE, &[])
                .unwrap_err()
                .contains("WIT type package")
        );
        let mut component = encode(&resolve, world);
        component.pop();
        assert!(validate_component(&component, PURE, &[]).is_err());
    }
}

#[cfg(test)]
mod ticket_tests {
    use super::*;
    #[test]
    fn additive_ticket_world_cannot_be_claimed_as_an_old_component_world() {
        let (resolve, world) = contract("ticket-tool-plugin").unwrap();
        let mut core =
            wit_component::dummy_module(&resolve, world, wit_parser::ManglingAndAbi::Standard32);
        wit_component::embed_component_metadata(
            &mut core,
            &resolve,
            world,
            wit_component::StringEncoding::UTF8,
        )
        .unwrap();
        let bytes = wit_component::ComponentEncoder::default()
            .module(&core)
            .unwrap()
            .validate(true)
            .encode()
            .unwrap();
        let imports: Vec<_> = surface(&resolve, world, true)
            .unwrap()
            .into_keys()
            .collect();
        let checked = validate_component(
            &bytes,
            crate::plugin::PLUGIN_COMPONENT_TICKET_TOOL_WORLD,
            &imports,
        )
        .unwrap();
        assert!(checked.imports.contains(&"yoi:host/objects@2.1.0".into()));
        assert!(
            checked
                .imports
                .contains(&"yoi:host/ticket-read@2.1.0".into())
        );
        assert!(
            checked
                .imports
                .contains(&"yoi:host/extension-data@2.1.0".into())
        );
        assert_eq!(checked.exports, ["yoi:plugin/tool@2.0.0"]);
        assert!(
            validate_component(
                &bytes,
                crate::plugin::PLUGIN_COMPONENT_HTTPS_LOGGED_TOOL_WORLD,
                &imports
            )
            .is_err()
        );
    }
}
