use std::sync::atomic::{AtomicU32, Ordering};
use yoi_plugin_pdk::{ToolError, wit_bindgen};
wit_bindgen::generate!({
    path: "../../../../resources/plugin/wit-service",
    world: "ticket-service-plugin",
    generate_all,
    runtime_path: "yoi_plugin_pdk::wit_bindgen::rt",
});
use exports::yoi::plugin::service::{Envelope, EventResult, TicketEventType};
use yoi::host2_0_0::{connections, errors, http, logging};
use yoi::host2_1_0::{extension_data as data, objects, ticket_read};

static CALLS: AtomicU32 = AtomicU32::new(0);
fn fresh() {
    assert_eq!(CALLS.fetch_add(1, Ordering::SeqCst), 0);
}
fn failure(e: errors::HostError) -> ToolError {
    ToolError::failed(format!("{:?}", e.code))
}
fn address() -> data::Address {
    data::Address {
        namespace: data::Collection {
            collection: "notes".into(),
            schema_id: "note".into(),
            schema_version: "1".into(),
        },
        key: "main".into(),
    }
}
fn request() -> http::Request {
    http::Request {
        method: http::Method::Post,
        path: "/events".into(),
        query: vec![],
        headers: vec![],
        body: b"event".to_vec(),
        timeout_ms: 15_000,
        effect_key: "event-hook".into(),
    }
}
fn lifecycle(mode: String) -> yoi_plugin_pdk::Result<()> {
    fresh();
    match mode.as_str() {
        "open" => {
            connections::open("api").map_err(failure)?;
        }
        "resolve" => {
            objects::resolve_ticket("T-796", objects::Access::Read).map_err(failure)?;
        }
        "read" => {
            let t = unsafe { objects::Ticket::from_handle(0) };
            ticket_read::read(&t).map_err(failure)?;
        }
        "data" => {
            let t = unsafe { objects::Ticket::from_handle(0) };
            data::get(&t, &address()).map_err(failure)?;
        }
        "http" => {
            let c = unsafe { connections::Connection::from_handle(0) };
            http::send(&c, &request()).map_err(failure)?;
        }
        "loop" => loop {
            std::hint::spin_loop();
        },
        _ => {
            logging::log(&logging::Entry {
                level: logging::Level::Info,
                event_code: "lifecycle".into(),
                message: "configured".into(),
                fields: vec![],
            })
            .map_err(failure)?;
        }
    }
    Ok(())
}
fn start(config: String) -> yoi_plugin_pdk::Result<()> {
    let config: std::collections::BTreeMap<String, String> =
        yoi_plugin_pdk::parse_json_input(&config)?;
    lifecycle(config.get("mode").cloned().unwrap_or_default())
}
fn stop(reason: String) -> yoi_plugin_pdk::Result<()> {
    lifecycle(reason)
}
fn work(input: &Envelope) -> yoi_plugin_pdk::Result<()> {
    if matches!(input.event_type, TicketEventType::TicketDeleted) {
        return Ok(());
    }
    let ticket = objects::resolve_ticket(&input.ticket_ref, objects::Access::ExtensionWrite)
        .map_err(failure)?;
    let snapshot = ticket_read::read(&ticket).map_err(failure)?;
    if input.config_json == r#"{"mode":"abi-probe"}"# {
        data::list(&ticket, &address().namespace, None, 1).map_err(failure)?;
        data::delete(&ticket, &address(), "observed", "delete").map_err(failure)?;
    }
    // A saved mutation is checked before reading the current cell or rebuilding
    // its CAS request. Failed lookup is never interpreted as not-recorded.
    match data::lookup_mutation(&ticket, &address(), "event-note").map_err(failure)? {
        data::MutationLookup::Committed(_) => (),
        data::MutationLookup::Expired => return Err(ToolError::failed("expired")),
        data::MutationLookup::NotRecorded => {
            let expected = match data::get(&ticket, &address()).map_err(failure)? {
                data::Cell::NeverWritten => data::Expectation::NeverWritten,
                data::Cell::Deleted(last) => data::Expectation::LastWrite(last),
                data::Cell::Present(cell) => data::Expectation::LastWrite(cell.last_write),
            };
            let value = yoi_plugin_pdk::extension_data_json(&snapshot.title)?;
            data::put(&ticket, &address(), &expected, "event-note", &value).map_err(failure)?;
        }
    }
    let connection = connections::open("api").map_err(failure)?;
    // Repeating this exact request uses the Host receipt, not a new effect key.
    http::send(&connection, &request()).map_err(failure)?;
    Ok(())
}
fn event(input: Envelope) -> EventResult {
    fresh();
    if input.config_json == r#"{"mode":"loop"}"# {
        loop {
            std::hint::spin_loop();
        }
    }
    if input.config_json == r#"{"mode":"reject"}"# {
        return EventResult::Reject("rejected".into());
    }
    if input.config_json == r#"{"mode":"oversize-retry"}"# {
        return EventResult::RetryAfter(300_001);
    }
    if work(&input).is_err() && input.config_json != r#"{"mode":"swallow-unknown"}"# {
        EventResult::RetryAfter(10)
    } else {
        EventResult::Ack
    }
}
yoi_plugin_pdk::export_component_service!(ServiceGuest, start, event, stop);
