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

/// Service exports reuse the existing Host resources and their exact import ABI.
pub mod service_bindings {
    wasmtime::component::bindgen!({
        path: "../../resources/plugin/wit-service",
        world: "ticket-service-plugin",
        imports: { default: async | trappable },
        exports: { default: async },
        with: {
            "yoi:host/errors@2.0.0": crate::bindings::yoi::host::errors,
            "yoi:host/connections@2.0.0": crate::bindings::yoi::host::connections,
            "yoi:host/http@2.0.0": crate::bindings::yoi::host::http,
            "yoi:host/logging@2.0.0": crate::bindings::yoi::host::logging,
            "yoi:host/objects@2.1.0": crate::host::objects,
            "yoi:host/ticket-read@2.1.0": crate::host::ticket_read,
            "yoi:host/extension-data@2.1.0": crate::host::extension_data,
        },
    });
}
