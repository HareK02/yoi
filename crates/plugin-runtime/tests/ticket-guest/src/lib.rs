use yoi_plugin_pdk::{ToolContext, ToolError, ToolOutput, wit_bindgen};
wit_bindgen::generate!({
    path: "../../../../resources/plugin/wit-ticket",
    world: "ticket-tool-plugin",
    generate_all,
    runtime_path: "yoi_plugin_pdk::wit_bindgen::rt",
});
use yoi::host2_1_0::{extension_data as data, objects, ticket_read};

fn namespace() -> data::Collection {
    data::Collection {
        collection: "notes".into(),
        schema_id: "note".into(),
        schema_version: "1".into(),
    }
}
fn address() -> data::Address {
    data::Address {
        namespace: namespace(),
        key: "main".into(),
    }
}
fn failure(error: yoi::host2_0_0::errors::HostError) -> ToolError {
    ToolError::failed(format!("{:?}", error.code))
}
fn handle(_: ToolContext, mode: String) -> yoi_plugin_pdk::Result<ToolOutput> {
    let ticket = if mode == "foreign" {
        unsafe { objects::Ticket::from_handle(0) }
    } else {
        objects::resolve_ticket("T-795", objects::Access::ExtensionWrite).map_err(failure)?
    };
    let ticket = if mode == "stale" {
        let handle = ticket.handle();
        drop(ticket);
        unsafe { objects::Ticket::from_handle(handle) }
    } else {
        ticket
    };
    let snapshot = ticket_read::read(&ticket).map_err(failure)?;
    if mode == "other-schema" {
        let mut a = address();
        a.namespace.schema_version = "2".into();
        data::get(&ticket, &a).map_err(failure)?;
    }
    if mode == "read" {
        return ToolOutput::json(snapshot.title, ());
    }
    let json = if mode == "bad-schema" {
        "{\"undeclared\":true}".into()
    } else {
        yoi_plugin_pdk::extension_data_json(&"note")?
    };
    let written = data::put(
        &ticket,
        &address(),
        &data::Expectation::NeverWritten,
        "put",
        &json,
    )
    .map_err(failure)?;
    assert!(matches!(
        data::get(&ticket, &address()).map_err(failure)?,
        data::Cell::Present(_)
    ));
    let page = data::list(&ticket, &namespace(), None, 10).map_err(failure)?;
    assert_eq!(page.entries.len(), 1);
    let lookup = data::lookup_mutation(&ticket, &address(), "put").map_err(failure)?;
    assert!(matches!(lookup, data::MutationLookup::Committed(_)));
    let deleted =
        data::delete(&ticket, &address(), &written.mutation_id, "delete").map_err(failure)?;
    assert!(matches!(
        data::get(&ticket, &address()).map_err(failure)?,
        data::Cell::Deleted(_)
    ));
    data::put(
        &ticket,
        &address(),
        &data::Expectation::LastWrite(deleted.mutation_id),
        "recreate",
        &json,
    )
    .map_err(failure)?;
    // Replay the original exact request even though the cell has since changed.
    let replay = data::put(
        &ticket,
        &address(),
        &data::Expectation::NeverWritten,
        "put",
        &json,
    )
    .map_err(failure)?;
    assert_eq!(replay.mutation_id, written.mutation_id);
    ToolOutput::json(snapshot.title, ())
}
yoi_plugin_pdk::export_ticket_component_tool!(TicketPlugin, handle);
