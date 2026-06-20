use serde_json::{json, Value};
use yoi_plugin_pdk::wit_bindgen;
use yoi_plugin_pdk::{export_plugin_instance, Plugin, PluginIngressEvent, PluginStatus, ToolOutput};

wit_bindgen::generate!({
    world: "instance",
    path: "../../../../resources/plugin/wit",
    generate_all,
    runtime_path: "yoi_plugin_pdk::wit_bindgen::rt",
});

struct ExamplePlugin {
    calls: u64,
}

impl Plugin for ExamplePlugin {
    fn start(_config: Value) -> yoi_plugin_pdk::Result<Self> {
        Ok(Self { calls: 0 })
    }

    fn handle_tool(&mut self, name: &str, input: Value) -> yoi_plugin_pdk::Result<ToolOutput> {
        self.calls += 1;
        ToolOutput::json(
            format!("{name} handled by shared instance"),
            json!({
                "tool": name,
                "calls": self.calls,
                "input": input
            }),
        )
    }

    fn handle_ingress(
        &mut self,
        name: &str,
        event: PluginIngressEvent,
    ) -> yoi_plugin_pdk::Result<Value> {
        Ok(json!({
            "ingress": name,
            "kind": event.kind,
            "source": event.source,
            "calls": self.calls,
            "accepted": true
        }))
    }

    fn status(&self) -> yoi_plugin_pdk::Result<PluginStatus> {
        Ok(PluginStatus::ready(json!({ "calls": self.calls })))
    }
}

export_plugin_instance!(ExamplePluginComponent, ExamplePlugin);
