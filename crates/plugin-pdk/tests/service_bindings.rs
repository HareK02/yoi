use yoi_plugin_pdk::{ToolError, wit_bindgen};
wit_bindgen::generate!({
    world: "ticket-service-plugin",
    path: "../../resources/plugin/wit-service",
    generate_all,
    runtime_path: "yoi_plugin_pdk::wit_bindgen::rt",
});
fn start(config: String) -> Result<(), ToolError> {
    if config == "invalid" {
        Err(ToolError::invalid_input("bad config"))
    } else {
        Ok(())
    }
}
fn event(_: exports::yoi::plugin::service::Envelope) -> exports::yoi::plugin::service::EventResult {
    exports::yoi::plugin::service::EventResult::RetryAfter(20)
}
fn stop(_: String) -> Result<(), ToolError> {
    Ok(())
}
yoi_plugin_pdk::export_component_service!(ServiceProbe, start, event, stop);

#[test]
fn service_macro_preserves_lifecycle_error_arm() {
    use exports::yoi::plugin::service::Guest;
    assert_eq!(
        ServiceProbe::on_start("invalid".into()).unwrap_err().code,
        "invalid_input"
    );
    ServiceProbe::on_stop("shutdown".into()).unwrap();
}
#[test]
fn service_dependencies_preserve_exact_older_pins() {
    assert_eq!(
        yoi_plugin_pdk::HOST_WIT,
        include_str!("../../../resources/plugin/wit-service/deps/yoi-host-v2/yoi-host-v2.wit")
    );
    assert_eq!(
        yoi_plugin_pdk::TICKET_HOST_WIT,
        include_str!("../../../resources/plugin/wit-service/deps/yoi-host-v2.1/yoi-host-v2.1.wit")
    );
    assert_eq!(
        yoi_plugin_pdk::SERVICE_WORLD,
        "yoi:plugin/ticket-service-plugin@2.2.0"
    );
    assert_eq!(
        yoi_plugin_pdk::SERVICE_WIT,
        include_str!("../../../resources/plugin/wit-service/yoi-plugin-v2.2.wit")
    );
}
