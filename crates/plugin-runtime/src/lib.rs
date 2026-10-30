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
pub mod owner;
mod sandbox;
pub use executor::*;

/// Additive Ticket imports share the original v2 error types.
pub mod ticket_bindings {
    wasmtime::component::bindgen!({
        path: "../../resources/plugin/wit-ticket",
        world: "ticket-tool-plugin",
        imports: { default: async | trappable },
        exports: { default: async },
        with: {
            "yoi:host/errors@2.0.0": crate::bindings::yoi::host::errors,
            "yoi:host/objects@2.1.0/ticket": crate::host::Ticket,
        },
    });
}
