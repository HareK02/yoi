//! Compile-pass: the additive world uses the original Tool export and typed
//! imported Ticket resources; no handwritten canonical ABI or ambient authority.
use yoi_plugin_pdk::wit_bindgen;
wit_bindgen::generate!({
    world: "ticket-tool-plugin",
    path: "../../resources/plugin/wit-ticket",
    generate_all,
    runtime_path: "yoi_plugin_pdk::wit_bindgen::rt",
});
fn handle(
    _: yoi_plugin_pdk::ToolContext,
    input: String,
) -> yoi_plugin_pdk::Result<yoi_plugin_pdk::ToolOutput> {
    yoi_plugin_pdk::ToolOutput::summary(input)
}
yoi_plugin_pdk::export_ticket_component_tool!(TicketProbe, handle);

#[test]
fn additive_export_uses_original_tool_result_abi() {
    use exports::yoi::plugin2_0_0::tool::{Guest, Invocation};
    let result = TicketProbe::invoke(Invocation {
        invocation_id: "invocation".into(),
        tool_name: "probe".into(),
        input_json: "\"ok\"".into(),
        config_json: "{}".into(),
        remaining_ms: 1000,
    })
    .unwrap();
    assert_eq!(result.summary, "ok");
    let _access = yoi::host2_1_0::objects::Access::ExtensionWrite;
}

#[test]
fn old_wit_dependencies_are_exact_copies_not_augmented_versions() {
    assert_eq!(
        include_str!("../../../resources/plugin/wit/yoi-plugin-v2.wit"),
        include_str!("../../../resources/plugin/wit-ticket/deps/yoi-plugin-v2/yoi-plugin-v2.wit")
    );
    assert_eq!(
        include_str!("../../../resources/plugin/wit/deps/yoi-host/yoi-host-v2.wit"),
        include_str!("../../../resources/plugin/wit-ticket/deps/yoi-host-v2/yoi-host-v2.wit")
    );
    assert_ne!(
        yoi_plugin_pdk::TICKET_TOOL_WORLD,
        yoi_plugin_pdk::TOOL_WORLD
    );
}
