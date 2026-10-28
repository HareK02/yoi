use yoi_plugin_pdk::wit_bindgen;
wit_bindgen::generate!({
    path: "../../../../resources/plugin/wit",
    world: "https-logged-tool-plugin",
    generate_all,
    runtime_path: "yoi_plugin_pdk::wit_bindgen::rt",
});
use exports::yoi::plugin::tool::{Guest, Invocation, Output, ToolError};
use std::sync::atomic::{AtomicU32, Ordering};
use yoi::host::{connections, http, logging};

static CALLS: AtomicU32 = AtomicU32::new(0);
struct Plugin;
impl Guest for Plugin {
    fn invoke(input: Invocation) -> Result<Output, ToolError> {
        let count = CALLS.fetch_add(1, Ordering::SeqCst) + 1;
        let mode = input.input_json.trim_matches('"');
        match mode {
            "trap" => panic!("guest trap"),
            "loop" => loop {
                std::hint::black_box(CALLS.load(Ordering::SeqCst));
            },
            "wall-loop" => {
                logging::log(&logging::Entry {
                    level: logging::Level::Info,
                    event_code: "count".into(),
                    message: "loop entered".into(),
                    fields: vec![],
                })
                .map_err(failure)?;
                loop {
                    std::hint::black_box(CALLS.load(Ordering::SeqCst));
                }
            }
            "memory" => {
                core::arch::wasm32::memory_grow::<0>(8192);
            }
            "error-output" => {
                return Err(ToolError {
                    code: "typed".into(),
                    message: "\"".repeat(40_000),
                });
            }
            "typed" | "unknown" => {
                return Err(ToolError {
                    code: mode.into(),
                    message: "bounded business failure".into(),
                });
            }
            "logging" => {
                for _ in 0..3 {
                    logging::log(&logging::Entry {
                        level: logging::Level::Info,
                        event_code: "count".into(),
                        message: "safe diagnostic".into(),
                        fields: vec![logging::Field {
                            name: "calls".into(),
                            value: logging::Scalar::Unsigned(count as u64),
                        }],
                    })
                    .map_err(failure)?;
                }
            }
            "http" | "stale" | "foreign" | "resources" => {
                if mode == "resources" {
                    let _resources: Vec<_> =
                        (0..64).map(|_| connections::open("api").unwrap()).collect();
                }
                let target = if mode == "foreign" {
                    // An attacker can fabricate a representation, not a capability.
                    unsafe { connections::Connection::from_handle(0) }
                } else {
                    connections::open("api").map_err(failure)?
                };
                let target = if mode == "stale" {
                    let handle = target.handle();
                    drop(target);
                    unsafe { connections::Connection::from_handle(handle) }
                } else {
                    target
                };
                let response = http::send(
                    &target,
                    &http::Request {
                        method: http::Method::Get,
                        path: "/status".into(),
                        query: vec![],
                        headers: vec![],
                        body: vec![],
                        timeout_ms: 15_000,
                        effect_key: "one".into(),
                    },
                )
                .map_err(failure)?;
                return Ok(Output {
                    summary: String::from_utf8(response.body).unwrap(),
                    content: None,
                    structured_json: Some(format!("{{\"calls\":{count}}}")),
                });
            }
            _ => (),
        }
        Ok(Output {
            summary: if mode == "output" {
                "x".repeat(70_000)
            } else {
                input.invocation_id
            },
            content: None,
            structured_json: Some(if mode == "bad-json" {
                "not-json".into()
            } else if mode == "bad-schema" {
                "{}".into()
            } else {
                format!("{{\"calls\":{count}}}")
            }),
        })
    }
}
fn failure(error: yoi::host::errors::HostError) -> ToolError {
    ToolError {
        code: "host".into(),
        message: format!(
            "{}:{}",
            match error.code {
                yoi::host::errors::Code::DeadlineExceeded => "deadline-exceeded",
                yoi::host::errors::Code::QuotaExceeded => "quota-exceeded",
                _ => "host-rejected",
            },
            match error.effect {
                yoi::host::errors::EffectObservation::NotDispatched => "not-dispatched",
                yoi::host::errors::EffectObservation::ResponseObserved => "response-observed",
                yoi::host::errors::EffectObservation::Indeterminate => "indeterminate",
            }
        ),
    }
}
export!(Plugin);
