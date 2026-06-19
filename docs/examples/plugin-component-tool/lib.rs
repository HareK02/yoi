//! Minimal Component Model Tool plugin authoring sketch.
//!
//! Build this as a `wasm32-unknown-unknown` cdylib with `wit-bindgen`-generated
//! exports and package the adapted component as `plugin.component.wasm`.

wit_bindgen::generate!({
    world: "tool",
    path: "../../../resources/plugin/wit",
});

struct Plugin;

impl Guest for Plugin {
    fn call(tool_name: String, input_json: String) -> String {
        // Ordinary ToolOutput JSON. The runtime routes this through the normal
        // Worker/Tool result path; no context is injected by the component.
        format!(
            r#"{{"summary":"component tool {tool_name}","content":"input was {input_json}"}}"#
        )
    }
}

export!(Plugin);
