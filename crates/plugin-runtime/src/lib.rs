#![doc = include_str!("../README.md")]

pub mod bindings {
    wasmtime::component::bindgen!({
        path: "../../resources/plugin/wit",
        world: "https-logged-tool-plugin",
        imports: { default: async | trappable },
        exports: { default: async },
        with: { "yoi:host/connections/connection": crate::host::Connection },
    });
}

pub mod delivery;
mod executor;
pub mod host;
mod sandbox;
pub use executor::*;
